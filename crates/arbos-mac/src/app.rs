//! `AppState`: the one shared entity. Views observe it (`cx.observe`) and
//! subscribe to the narrow [`AppEvent::TranscriptText`] for streaming appends
//! (zeron-study §4.4 step 4). It owns the sync channel; nothing here touches
//! GPUI elements.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use arbos_matrix::{ChannelRow, MessageRow, Snapshot, SpaceRow};
use gpui::{App, Context, EventEmitter, RenderImage, Task};
use gpui_tokio::Tokio;
use tokio::sync::mpsc;

use crate::auth;
use crate::calls::{self, CallCmd, CallEvent};
use crate::chat_search;
use crate::model::SpaceKind;
use crate::model::{
    add_members_plan, added_line, call_ended_line, created_group, reachable_line,
    reconcile_optimistic_notices, trace_rows, AccountView, Call, CallEnded, CallKind, CallMissed,
    MembershipNotice, NoticeSubject, OptimisticNotice, Person, Reachable, Sender, Voice,
};
use crate::model::{
    new_id, now_ms, Attachment, AttachmentKind, AttachmentSource, BoxReport, BoxState, ChannelSel,
    ChannelsView, Chat, ChatKind, ConnectorsView, DevicesView, Effort, GifPage, Id, Job,
    JobRollout, Link, LinkState, LocalView, Machine, MediaPick, Message, MessageBody,
    MessageStatus, NewSecret, Part, Project, ProjectDocView, ProjectSecrets, QuestionState,
    ReplyRef, Role, Routine, RoutineRun, RoutineTrigger, SecretDirector, SecretScope, SecretsView,
    ServerInfo, Skill, StatusKind, StickerPack,
};
use crate::palette;
use crate::search;
use crate::search_index::{self, SearchIndex};
use crate::sign_in;
use crate::sync::user_error::{ErrorClass, UserError};
use crate::sync::{ClientCommand, PushRefusal, SyncEvent, SyncSource};
use crate::ui::error_note;
use crate::ui::notify::{self, Gate, Notifier, NotifyEvent, NotifyKind};
use crate::ui::tones::{SamplePlayer, Tone, TonePlayer};

/// How often held notification batches are re-checked.
const NOTIFY_TICK_MS: u64 = 5_000;
/// How often the open chat's background jobs are polled (`GET /projects/{id}`
/// → `activity.jobs[]`) while a job row is live or the drawer lists one —
/// the design's "chip updates on the next poll" (duplex §2.4, stop ≤ 3 s).
pub const JOBS_POLL_MS: u64 = 1_000;
/// Sidebar pins the relay allows until `GET /me/pins` says otherwise (Jacob 2026-10-28:
/// 5, no tiers; [feedback sidebar-pinning-mac-2026-10-28]).
pub const PIN_CAP_DEFAULT: usize = 5;
/// A link state must hold this long before the header pill shows it.
pub const LINK_PILL_GRACE_MS: u64 = 1_200;
/// The ring card and its sound give up after this long.
const RING_TIMEOUT_MS: u64 = 45_000;
/// An outgoing call nobody answers is hung up after this long.
const RINGBACK_TIMEOUT_MS: u64 = 60_000;
/// How long a `call_missed` push waits for its chat to load before it is dropped.
const MISSED_PENDING_MS: u64 = 120_000;
/// How long the original message stays highlighted after a click on a reply's quote.
const QUOTE_HIGHLIGHT_MS: u64 = 1_600;

/// Which profile sheet is open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfilePage {
    /// A person, by handle; `space_id` = the chat it was opened from.
    User {
        handle: String,
        space_id: Option<Id>,
    },
    /// A people group chat or a shared project.
    Group { space_id: Id },
    /// An agent (one of my projects or a shared one).
    Agent { project_id: Id },
    /// My own profile and every app setting (the Settings window is gone).
    Me,
    /// A server (D88): channels, members, and the admin options for owners/admins.
    Server { server_id: Id },
    /// A channel of a server (D88, #1317): what the `# general` header pill opens.
    Channel { server_id: Id, channel_id: Id },
}

impl ProfilePage {
    /// True when both pages show the same person, agent, group, server or
    /// channel. A person's page is the same person whichever chat it was
    /// opened from (`space_id` only says where the Message pill goes), so a
    /// click on their sender name, their @mention chip or the DM header pill
    /// all count as "that profile is showing" ([feedback profile-pane-toggle-mac-2026-10-26]).
    pub fn same_subject(&self, other: &ProfilePage) -> bool {
        match (self, other) {
            (ProfilePage::User { handle: a, .. }, ProfilePage::User { handle: b, .. }) => {
                a.eq_ignore_ascii_case(b)
            }
            _ => self == other,
        }
    }
}

/// Who the Add members picker adds to — a group or a server (D88) — and who is in already.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddTarget {
    pub name: String,
    /// Handles already in (mine included): their rows show greyed and checked.
    pub handles: Vec<String>,
    /// Agents already in (groups only; servers have none).
    pub agents: Vec<Id>,
    /// People only: no agent rows (servers, and a secret's People list).
    pub is_server: bool,
    /// Opened from a Secret page's "Give a person access…" (spec §7.7): the sheet is titled
    /// for that and Add puts the picks on the secret's People list.
    pub is_secret: bool,
}

/// Which server sheet is open over the shell (D88).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerSheet {
    /// `+` → "New server": one name field.
    NewServer,
    /// `+` → "Join a server": paste an invite link or code.
    JoinServer,
    /// `+` inside a server: channel name and the "Admins only post" toggle.
    NewChannel { server_id: Id },
}

/// A page under the open one in the right pane's stack (a group under the
/// member opened from it), kept with what `‹` needs to bring it back as it
/// was: its tab and how far its body was scrolled (px from the top).
#[derive(Clone, Debug, PartialEq)]
pub struct ProfileFrame {
    pub page: ProfilePage,
    pub tab: usize,
    pub scroll: f32,
}

/// Which agent-settings row on the agent's profile page is expanded into its
/// choice list (click to open; see `ui::hoverpick`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentPick {
    Model,
    Reasoning,
    Voice,
}

impl AgentPick {
    /// `ARBOS_OPEN_PROFILE=agent:<model|reasoning|voice>` (rig).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "model" => Some(Self::Model),
            "reasoning" | "effort" => Some(Self::Reasoning),
            "voice" => Some(Self::Voice),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Kept for the relay path; the person pane (2026-10-20) and the agent pane
/// (2026-10-21) no longer offer a call button — calls start from the chat.
#[allow(dead_code)]
pub enum CallTarget {
    User(String),
    Agent(Id),
}

fn muted_path() -> PathBuf {
    crate::sync::credentials::config_dir().join("muted.json")
}

fn read_receipts_path() -> PathBuf {
    crate::sync::credentials::config_dir().join("read_receipts.json")
}

/// Multi-chat (several chats per agent: tabs, "+", ⌘T, archive) is on by
/// default. `ARBOS_MULTI_CHAT=0` or `{"multi_chat": false}` in
/// `<config>/settings.json` turns it off (one chat per agent).
fn multi_chat_enabled() -> bool {
    if let Ok(v) = std::env::var("ARBOS_MULTI_CHAT") {
        return !matches!(v.trim(), "0" | "false" | "no" | "off");
    }
    std::fs::read_to_string(crate::sync::credentials::config_dir().join("settings.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("multi_chat").and_then(|b| b.as_bool()))
        .unwrap_or(true)
}

fn load_muted() -> HashSet<Id> {
    std::fs::read_to_string(muted_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_muted(muted: &HashSet<Id>) {
    let dir = crate::sync::credentials::config_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(json) = serde_json::to_string(&muted.iter().collect::<Vec<_>>()) {
        let _ = std::fs::write(muted_path(), json);
    }
}

/// Settings → Notifications "Read receipts" (default on, matching the relay).
fn load_read_receipts() -> bool {
    std::fs::read_to_string(read_receipts_path())
        .ok()
        .and_then(|s| serde_json::from_str::<bool>(&s).ok())
        .unwrap_or(true)
}

fn save_read_receipts(enabled: bool) {
    let dir = crate::sync::credentials::config_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(read_receipts_path(), if enabled { "true" } else { "false" });
}

fn link_previews_path() -> std::path::PathBuf {
    crate::sync::credentials::config_dir().join("link-previews")
}

/// Settings → Chat → Link previews, saved on this Mac; on unless switched off
/// ([feedback link-embeds-mac-2026-10-27]).
fn load_link_previews() -> bool {
    std::fs::read_to_string(link_previews_path())
        .ok()
        .map(|s| s.trim() != "false")
        .unwrap_or(true)
}

fn save_link_previews(enabled: bool) {
    let dir = crate::sync::credentials::config_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(link_previews_path(), if enabled { "true" } else { "false" });
}

/// "3 min ago", "2 h ago", "yesterday", "5 days ago".
pub fn ago_text(ms: u64) -> String {
    let s = ms / 1000;
    if s < 60 {
        "just now".into()
    } else if s < 3_600 {
        format!("{} min ago", s / 60)
    } else if s < 86_400 {
        format!("{} h ago", s / 3_600)
    } else if s < 2 * 86_400 {
        "yesterday".into()
    } else {
        format!("{} days ago", s / 86_400)
    }
}
/// A chat's first snapshot only announces messages younger than this.
const SNAPSHOT_FRESH_MS: u64 = 30_000;
use crate::update::{self, UpdateCommand, UpdateEvent, UpdatePhase, UpdateState};

/// How the sign-in screen signs this device in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignInMode {
    /// "Sign in with GitHub" through the relay's OAuth flow.
    GitHub,
    /// The relay runs `ARBOS_AUTH_MODE=paircode`: one-time code from its log.
    PairCode,
}

/// Sign-in screen state (first launch, after sign-out, after a revoked token).
#[derive(Clone, Debug)]
pub struct Pairing {
    pub relay_url: String,
    pub mode: SignInMode,
    /// Pairing-code mode: the code is being exchanged.
    pub busy: bool,
    /// Pairing-code mode: the last failure.
    pub error: Option<UserError>,
    /// The sign-in note's Details disclosure (either mode).
    pub error_details_open: bool,
    /// GitHub mode: the pill's state machine (copy, Cancel / Retry, timeout).
    pub flow: sign_in::Flow,
}

/// A downloaded blob shown in the Details drawer.
#[derive(Clone, Debug)]
pub struct BlobView {
    pub mime: String,
    pub bytes: Arc<Vec<u8>>,
    pub saved_to: Option<PathBuf>,
}

/// A local refusal (the form is not complete): the shared element with the words as the why.
pub fn refused(verb: &str, why: &str) -> error_note::Slot {
    error_note::Slot::new(UserError::of(
        verb,
        ErrorClass::Refused {
            message: Some(why.to_owned()),
        },
        "",
    ))
}

/// Which surface owns which failures while it is up (see `AppState::failure_claim`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailureClaim {
    pub surface: &'static str,
    pub verbs: &'static [&'static str],
}

/// A toast in the one slot under the header pill: an info notice (`error` none: "Forwarded to
/// Studio") or a failure drawn by `ui::error_note` (Jacob, 2026-10-26 / 2026-10-27). Info
/// notices live 6 s, errors 10 s; the pointer on a toast holds it (`held`, at most
/// [`NOTICE_HOLD_MAX_MS`] unless Details is open), × or a click on an info toast dismisses it.
/// The same failure again while its toast stands does not add a toast: `count` goes up and the
/// clock restarts (eight "Couldn't load your secrets" in a row were eight toasts on
/// 2026-10-27 — never again). `id` is stable for the toast's life; `at` is its clock.
#[derive(Clone, Debug)]
pub struct Notice {
    pub id: u64,
    pub text: String,
    pub at: u64,
    pub error: Option<UserError>,
    pub details_open: bool,
    pub held: bool,
    /// Since when the pointer holds it (the hold has a cap).
    pub held_since: u64,
    /// Try again re-issues this command.
    pub retry: Option<Box<ClientCommand>>,
    /// How many times this same failure arrived while the toast stood (1 = once).
    pub count: u32,
}

impl Notice {
    pub fn ttl_ms(&self) -> u64 {
        if self.error.is_some() {
            ERROR_NOTICE_TTL_MS
        } else {
            NOTICE_TTL_MS
        }
    }

    /// Still on screen at `now`: within its TTL, or held — a hold without Details open
    /// lasts at most [`NOTICE_HOLD_MAX_MS`], so a pointer parked over the slot (or a hover
    /// whose leave never arrived) cannot pin a toast forever.
    pub fn live_at(&self, now: u64) -> bool {
        if self.held
            && (self.details_open || now.saturating_sub(self.held_since) < NOTICE_HOLD_MAX_MS)
        {
            return true;
        }
        now.saturating_sub(self.at) < self.ttl_ms()
    }

    /// Two toasts are the same failure when they would read the same sentence.
    fn same_failure(&self, error: &UserError) -> bool {
        self.error
            .as_ref()
            .is_some_and(|e| e.sentence() == error.sentence())
    }
}

pub const NOTICE_TTL_MS: u64 = 6_000;
pub const ERROR_NOTICE_TTL_MS: u64 = 10_000;
/// A held toast (pointer on it, Details closed) is let go after this long.
pub const NOTICE_HOLD_MAX_MS: u64 = 60_000;
/// How many toasts the slot shows at once; older ones fold into one "N more" line
/// (Jacob 2026-10-27: "max 2–3 visible, oldest collapses into a count").
pub const NOTICE_VISIBLE_MAX: usize = 3;

/// What the slot draws: the newest toasts, oldest first, and how many older ones are folded
/// away behind them.
#[derive(Clone, Debug, Default)]
pub struct NoticeStack {
    pub visible: Vec<Notice>,
    pub folded: usize,
}

#[derive(Clone, Debug)]
pub enum AppEvent {
    /// Pure text append on a streaming message; only the transcript re-syncs.
    TranscriptText { chat_id: Id },
    /// Anything else in a chat changed.
    TranscriptChanged { chat_id: Id },
    /// F11: speech-to-text finished for a `transcribe` request.
    VoiceTranscript {
        request_id: u64,
        text: String,
        error: Option<String>,
        mock: bool,
    },
    /// F11: text-to-speech finished for a `speak` request.
    VoiceSpeech {
        request_id: u64,
        bytes: Arc<Vec<u8>>,
        mime: String,
        error: Option<String>,
        mock: bool,
    },
    /// F11: the speaker icon on a reply was clicked.
    ReplaySpeech { message_id: Id },
}

/// One entry of the media viewer's album (feedback media-viewer-2026-10-23).
#[derive(Clone, Debug)]
pub struct ViewerItem {
    pub attachment: Attachment,
    /// Who shared it ("Samir Haddad", "You", the agent's name).
    pub by: String,
    pub when: u64,
    pub chat_id: Id,
    /// The `parts[]` id, so a card click lands on its own picture.
    pub part_id: Id,
    /// A profile picture (Jacob, 2026-10-25: tap the hero to view it): the bytes come from the
    /// avatar cache for this URL, not from a chat blob — no Download / Forward, caption "Profile
    /// picture".
    pub picture_url: Option<String>,
}

impl ViewerItem {
    /// `name`'s profile picture at `url`, as one viewer page.
    pub fn picture(name: &str, url: &str) -> Self {
        ViewerItem {
            attachment: Attachment {
                name: "Profile picture".into(),
                mime: "image/jpeg".into(),
                blob_ref: None,
                bytes: None,
                kind: Some(AttachmentKind::Image),
                title: None,
                chart_part: None,
                url: None,
                source: None,
            },
            by: name.to_owned(),
            when: 0,
            chat_id: String::new(),
            part_id: String::new(),
            picture_url: Some(url.to_owned()),
        }
    }
}

/// The open media viewer: an album, the page shown, where its blobs live.
#[derive(Clone, Debug)]
pub struct MediaViewer {
    pub items: Vec<ViewerItem>,
    pub index: usize,
    /// Blob slot the items are read from (the selected space's id).
    pub slot: Id,
    /// The Forward picker is up.
    pub forward_open: bool,
    /// Blob key being fetched for Play / Quick Look (one at a time).
    pub busy: Option<String>,
    /// Temp file of the video playing inline in the viewer (macOS AVPlayerView;
    /// [feedback app-01M6147S9TPNBQA7KSDEY0K8C4]). Cleared on close / page.
    pub playing: Option<std::path::PathBuf>,
}

impl MediaViewer {
    pub fn current(&self) -> Option<&ViewerItem> {
        self.items.get(self.index)
    }
}

/// A chat the Forward picker can send to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForwardTarget {
    pub space_id: Id,
    pub chat_id: Id,
    pub name: String,
    /// "@sam", "Group · 4 members", "Agent".
    pub subtitle: String,
    pub last_activity: u64,
}

/// Bubble menu → Forward (feedback app-01M5MXRZ7609S70P9S9P2RZDDF): the
/// message the open picker will send on, and who wrote it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForwardMessage {
    pub chat_id: Id,
    pub message_id: Id,
    /// The original sender's display name ("Forwarded from <by>").
    pub by: String,
}

/// A Copy whose attachment bytes are being fetched
/// ([feedback copy-image-messages-mac-2026-10-27]): the body to lay out once they land.
#[derive(Clone, Debug, PartialEq)]
struct CopyRequest {
    id: u64,
    body: MessageBody,
}

/// The parts the optimistic row in the destination draws before the relay
/// echoes the real one: the same blob keys (their thumbnails are cached), the
/// chart parts with their fallback pictures, then the text
/// ([feedback forward-attachments-mac-2026-10-27]).
pub fn forward_optimistic_parts(body: &MessageBody) -> Vec<Part> {
    let mut parts: Vec<Part> = body
        .attachments
        .iter()
        .map(|a| Part::Attachment {
            id: new_id(),
            attachment: a.clone(),
        })
        .collect();
    for c in &body.charts {
        let Part::Chart {
            spec_version,
            chart,
            table,
            ..
        } = &c.chart
        else {
            continue;
        };
        let chart_id = new_id();
        let fallback_id = c.fallback.as_ref().map(|a| {
            let id = new_id();
            let mut attachment = a.clone();
            attachment.chart_part = Some(chart_id.clone());
            parts.push(Part::Attachment {
                id: id.clone(),
                attachment,
            });
            id
        });
        parts.push(Part::Chart {
            id: chart_id,
            spec_version: *spec_version,
            chart: chart.clone(),
            table: table.clone(),
            fallback: fallback_id,
        });
    }
    if !body.text.is_empty() {
        parts.push(Part::Text {
            id: new_id(),
            text: body.text.clone(),
        });
    }
    parts
}

/// The text line a forwarded attachment travels with (every client renders
/// it as the card's hint, not a bubble; agents read it as prose).
pub fn forwarded_from_text(by: &str) -> String {
    format!("{FORWARDED_FROM_PREFIX}{by}")
}

pub const FORWARDED_FROM_PREFIX: &str = "Forwarded from ";

/// The picker's list for what was typed, through the shared matcher
/// (`arbos_core::search`): the chat's name, its @handle (a DM's subtitle) or
/// its kind line ("Group · 4 members", "Agent") — best match first, most
/// recent inside a tie; an empty query keeps every chat by recency.
pub fn filter_forward_targets(mut targets: Vec<ForwardTarget>, query: &str) -> Vec<ForwardTarget> {
    use arbos_core::search::{normalize_query, rank, Fields};
    targets.sort_by_key(|t| std::cmp::Reverse(t.last_activity));
    let q = normalize_query(query);
    rank(targets, &q, |t| {
        let fields = Fields::new(t.name.clone(), "");
        match t.subtitle.strip_prefix('@') {
            Some(handle) => Fields {
                handle: handle.to_string(),
                ..fields
            },
            None => fields.with_title(t.subtitle.clone()),
        }
    })
    .into_iter()
    .map(|(t, _)| t)
    .collect()
}

/// What the composer should look like for the active chat (§3.10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposerMode {
    /// Send arrow. Sending is always allowed: the agent multitasks, and a
    /// running turn shows as a small indicator with its own Stop.
    Send,
    /// S8: the agent asked a question.
    Answer,
    /// S18: an approval card is pending; the composer waits for Approve/Reject.
    Approve,
}

/// Per-chat UI state kept across tab switches.
#[derive(Clone, Debug, Default)]
pub struct ChatUi {
    /// S3 → S9: the composer dropped to the bottom; it never returns.
    pub composer_dropped: bool,
    /// When the drop animation started (ms since epoch).
    pub drop_started_at: Option<u64>,
    /// Message the next send replies to (hover reply / people-chat parity).
    pub reply_to: Option<Id>,
    /// Reply in DM: the message in another chat the next send here answers
    /// (its address + snapshot, written as `replyRef`). Exclusive with `reply_to`.
    pub reply_ref: Option<ReplyRef>,
    /// Own message whose text is in the composer for an in-place edit
    /// (iPhone `ChatView.editing`). Exclusive with `reply_to` / `reply_ref`.
    pub editing: Option<Id>,
}

/// Rows per history page the shared Matrix client fetches (`arbos_matrix::client::HISTORY_PAGE`).
const CHANNEL_PAGE: usize = 50;

/// The Wake URL form on a connected agent's Connection card ([feedback
/// outside-agents-wake-mac-2026-10-28]): what is in flight and the last outcome line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WakeForm {
    pub project_id: Id,
    pub saving: bool,
    pub sending: bool,
    /// `(ok, text)` of the last Save or Send test wake, shown under the buttons.
    pub note: Option<(bool, String)>,
}

/// The reveal sheet after Connect an agent (#2141): the key, shown once, and the live
/// connection while the sheet is up.
#[derive(Clone, Debug, PartialEq)]
pub struct ConnectReveal {
    pub project_id: Id,
    pub name: String,
    /// `acp` | `grok` | `custom`.
    pub transport: String,
    pub key: String,
    pub link: String,
    pub relay_url: String,
    pub chat_id: Option<Id>,
    /// After Rotate key…: the sheet says so instead of the first-time words.
    pub rotated: bool,
    pub status: Option<crate::model::OutsideInfo>,
    /// Which Copy was pressed last ("key" | "link"), for the "Copied" tick.
    pub copied: Option<&'static str>,
}

/// What the Create group screen (step 1) hands to the member picker (step 2).
#[derive(Clone, Debug, PartialEq)]
pub struct GroupDraft {
    pub title: String,
    /// The leaf pair to store: the neutral pair since 2026-10-25 (no leaf grid on the sheet).
    pub leaf: Option<crate::ui::avatars::LeafPick>,
    /// The sheet's uploaded / generated picture, if any.
    pub picture: Option<crate::sync::DraftPicture>,
}

/// The relay's answer to the avatar picker's Generate tab.
#[derive(Clone, Debug)]
pub struct AvatarGenerated {
    pub target: crate::sync::PictureTarget,
    pub image_key: Option<String>,
    pub url: Option<String>,
    pub error: Option<UserError>,
}

/// Where a secret hand-off card's save stands on this device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandoffState {
    Saving,
    /// Saved; the vault name.
    Saved(String),
    /// The shared error note's slot (sentence + Try again + Details) on the card.
    Failed(crate::ui::error_note::Slot),
}

pub struct AppState {
    pub source_name: &'static str,
    pub pairing: Option<Pairing>,
    pub device_id: Option<String>,
    /// Signed-in account (`/auth/token` user, then `/me`).
    pub account: Option<AccountView>,
    /// The tree mark (the macOS app icon), drawn on the sign-in screen.
    pub mark: Option<Arc<RenderImage>>,
    /// People chats (D72): the new-chat sheet, directory results, typing, avatars.
    pub new_chat_open: bool,
    /// New Message → New group: multi-select people (and optional agents).
    pub new_group_open: bool,
    /// D93 Create group step 1 → 2: the name and picture chosen, sent with the members on Create.
    pub new_group_draft: Option<GroupDraft>,
    pub new_group_picked: Vec<Person>,
    pub new_group_agents: Vec<Id>,
    pub people_query: String,
    pub people_results: Vec<Person>,
    /// The people I block, as the relay lists them (`GET /blocks`; `SyncEvent::Blocked`).
    /// The person pane's Block user / Unblock row derives from this, never from a local flag.
    pub blocked: Vec<Person>,
    pub people_busy: bool,
    /// The New Message / New group / Add members sheets' note (the shared error element).
    pub people_error: Option<error_note::Slot>,
    /// Per chat: the last send / attachment / recording failure, drawn above the composer
    /// until Try again, a successful send, or the user dismisses it.
    pub send_errors: HashMap<Id, error_note::Slot>,
    /// Everyone on Arbos (`GET /users`), fetched once the account is known and
    /// on every sheet open: the New Message sheet paints it on its first frame
    /// instead of "Searching…" that the list then replaces.
    pub people_directory: Vec<Person>,
    /// Per chat: (handle, until ms) for everyone typing; the presence row over
    /// the composer reads it (`ui::presence`), newest first.
    pub typing: HashMap<Id, Vec<(String, u64)>>,
    /// Decoded avatars by `avatar_url`; `None` while a fetch is in flight.
    pub avatars: HashMap<String, Option<Arc<RenderImage>>>,
    /// The dominant colour of each decoded avatar (by URL; the relay's URL
    /// carries the user id and a version, so a new picture is a new key) for
    /// the bar on the sender's bubbles. Absent = colourless picture → D79.
    pub accents: HashMap<String, arbos_core::avatar::Accent>,
    /// A DM just started from the sheet: select it when it arrives.
    pub(crate) awaiting_dm: Option<String>,
    /// A DM with someone I have no chat with, not sent to yet: local only (`draft_dm`).
    pub(crate) draft_dm: Option<crate::draft_dm::DraftDm>,
    /// Reply in DM: the cross-chat reply to arm once the DM opens (new DMs
    /// arrive from the relay a moment after `start_dm`).
    pending_dm_reply: Option<ReplyRef>,
    /// A people group just created: select it when those handles appear.
    awaiting_group: Option<Vec<String>>,
    /// The sheet invites into this project instead of starting a DM (step 3).
    pub invite_target: Option<Id>,
    /// "Who may message X" (agent-to-agent messaging): the agent whose grant list a pick in
    /// the people sheet adds to.
    pub message_grant_target: Option<Id>,
    /// The group picker adds to this people group instead of creating one
    /// (the group pane's "Add members").
    pub add_members_target: Option<Id>,
    /// Server pane › Members › Add members (D88): the server the picks go into.
    pub add_members_server: Option<Id>,
    /// Secret page › People › "Give a person access…": the secret the picks go onto.
    pub add_members_secret: Option<Id>,
    /// An invite code handed over by `arbos://join/<code>`; the shell puts it in the Join sheet.
    pub pending_join_code: Option<String>,
    /// Avatar reply reaction window per project (ms): plays once after a turn.
    pub avatar_reply_until: HashMap<Id, u64>,
    /// Avatar picker open for this project.
    pub avatar_picker: Option<Id>,
    /// The open profile came from a single click on the header name (#1068):
    /// a second click of a slow double-click lands on its backdrop and is
    /// routed to the rename instead of closing it.
    pub profile_from_name: bool,
    /// Last time we told the room we are typing, per chat.
    typing_sent_at: HashMap<Id, u64>,
    /// Set by a fresh sign-in: with no projects yet, create "Home" and land in it.
    first_project_pending: bool,
    /// D82: the first-login screen while the relay creates the computer and the
    /// first agent; None on every ordinary launch (`onboarding.rs`).
    pub onboarding: Option<crate::onboarding::Onboarding>,
    pub projects: Vec<Project>,
    /// Picker contents; empty = the fixture's static list.
    pub models: Vec<String>,
    pub project_docs: HashMap<Id, ProjectDocView>,
    /// The skills library (F5), as last fetched from the source.
    pub skills: Vec<Skill>,
    pub chats: HashMap<Id, Vec<Chat>>,
    pub messages: HashMap<Id, Vec<Message>>,
    /// Membership notices this device shows ahead of the relay's row (ARB-393), per chat.
    pub optimistic_notices: HashMap<Id, Vec<OptimisticNotice>>,
    pub chat_ui: HashMap<Id, ChatUi>,
    pub blobs: HashMap<String, BlobView>,
    /// Decoded inline thumbnails of image attachments, by blob key (F10).
    pub thumbnails: HashMap<String, Arc<RenderImage>>,
    /// The command palette's local index over every chat doc on this Mac
    /// ([feedback search-palette-app-wide-2026-10-24]); kept current on each
    /// snapshot, upsert and channel page.
    pub search_index: SearchIndex,
    /// The same pictures with a uniform border trimmed (a screenshot's white
    /// page margin), for the chat's picture bubbles; absent when nothing to trim.
    pub thumbnails_trimmed: HashMap<String, Arc<RenderImage>>,
    /// Thumbnail fetches in flight or failed (never asked twice).
    thumbnails_requested: HashSet<String>,
    /// Site icons for the bubbles' named links, by host: `Some` once the relay answered
    /// with a picture, `None` when the site has none; a host still loading is absent.
    pub link_icons: HashMap<String, Option<Arc<RenderImage>>>,
    /// Each fetched link icon's tone (a dark transparent mark, a light one, or neither), so
    /// the link tiles can flip their fill under it ([`crate::ui::link_tile`]).
    pub link_icon_tones: HashMap<String, crate::ui::link_tile::Tone>,
    /// Link-icon fetches in flight or answered (never asked twice per session).
    link_icons_requested: HashSet<String>,
    /// Blob keys whose thumbnail fetch failed: the card says "No preview" instead of loading.
    pub thumbnails_failed: HashSet<String>,
    /// Picker (ARB-544): the GIF results on screen — the query they answer and the page
    /// (`None` while the first page is on its way).
    pub gif_results: Option<(String, GifPage)>,
    /// Picker: the query a GIF fetch is in flight for (a stale answer is dropped).
    gif_query_pending: Option<String>,
    /// Picker: the relay's sticker packs once fetched (`Some(None)` = the relay failed).
    pub sticker_packs: Option<Option<Vec<StickerPack>>>,
    /// Picker: the sticker-pack fetch is in flight or answered this session.
    sticker_packs_requested: bool,
    /// Picker: raw bytes of fetched stickers by `sticker:<pack>/<id>` key — a tap uploads
    /// these into the chat slot (the thumbnail cache holds only the decoded picture).
    sticker_raw: HashMap<String, Arc<Vec<u8>>>,
    /// The Copy whose attachment bytes are on their way
    /// ([feedback copy-image-messages-mac-2026-10-27]); a newer Copy replaces it and the
    /// older answer is dropped by its `request` number.
    copy_request: Option<CopyRequest>,
    copy_request_seq: u64,
    pub notices: Vec<Notice>,
    /// Rooms whose pushes were refused (`not_available` / `blocked`) and why; the chat-level
    /// note draws the sentence. Cleared when that room's next ACK arrives.
    pub push_refused: HashMap<Id, PushRefusal>,
    /// Last `silenced_until` we armed an end timer for (per people chat).
    silence_until_armed: HashMap<Id, i64>,
    /// Generation per chat so a re-armed silence-end timer makes the old one a no-op.
    silence_until_gen: HashMap<Id, u64>,
    pub selected_project: Option<Id>,
    /// Bytes and chats kept on this Mac by the on-device store (Settings → About).
    pub store_usage: (i64, i64),
    pub active_tab: HashMap<Id, Id>,
    /// Feature flag: several chats per agent. Off → one chat per agent, the
    /// primary one (`primary_chat()`); extra chats stay in the data, unseen.
    pub multi_chat: bool,
    /// Last tab opened per agent on this Mac (within a session). With
    /// multi-chat on, returning to an agent restores this tab; with it off,
    /// the rail opens `primary_chat` so history matches iPhone (D78).
    pub last_chats: HashMap<Id, Id>,
    /// Chats archived for me (§9c, synced through the relay; the store keeps an
    /// offline copy): off the tab strip, listed in the "+" dropdown, still open on
    /// the relay (history in context, memory and search), never badge or notify.
    pub archived_chats: HashSet<Id>,
    /// My sidebar pins, top first — row keys `agent:<project>` / `chat:<people chat>` /
    /// `server:<server>`, one list for every device through the relay (`/me/pins`;
    /// [feedback sidebar-pinning-mac-2026-10-28]). The Pinned section draws them in this
    /// order; `pin_cap` is the relay's limit (Jacob 2026-10-28: 5, no tiers).
    pub pins: Vec<String>,
    pub pin_cap: usize,
    /// Chat a home-search hit opened; kept visible while single-chat would hide it.
    search_chat: Option<Id>,
    /// Scroll the transcript to this `(chat_id, message_id)` once its rows exist.
    pub reveal_message: Option<(Id, Id)>,
    /// Bumped with every new reveal request, so revealing the same message
    /// twice (two clicks on one quote) scrolls twice.
    pub reveal_nonce: u64,
    /// Brief highlight on the message a home-search hit or a quote click opened.
    pub highlight_message: Option<Id>,
    /// ⌘F Search-in-chat ([feedback search-in-chat-mac-2026-10-26]): the open
    /// bar's query, hits and cursor for the chat it was opened in.
    pub chat_search: Option<ChatSearch>,
    pub link: Link,
    /// Each room's socket state (keyed like the docs: chat id, or the project
    /// id for the project doc) and when it last changed, for the header's
    /// status pill ([feedback connection-pill]).
    pub room_links: HashMap<Id, (LinkState, u64)>,
    /// The relay's HTTP reachability (registry poll) and when it last changed.
    pub relay_link: (LinkState, u64),
    pub rail_collapsed: bool,
    /// Chat-list column width (pt). Dragged on the right edge; persisted in
    /// `<config>/sidebar_width` ([feedback app-01M5MJ9Y91W9ZET71KTCE7PS65]).
    pub sidebar_width: f32,
    /// Height of the call strip at the top of the chat column while a call is live
    /// (`callview::strip_layout`); the handle under the strip drags it.
    pub call_strip_height: f32,
    /// The running edge drag's state (`ui::sidebar_fit::EdgeDrag`).
    pub sidebar_edge_drag: crate::ui::sidebar_fit::EdgeDrag,
    /// Below [`NARROW_WINDOW`](crate::ui::theme::NARROW_WINDOW) the sidebar
    /// auto-hides; the header toggle (or ⌘B) floats it over the chat instead.
    pub sidebar_overlay: bool,
    pub details_open: Option<DetailsTarget>,
    /// The media viewer (feedback media-viewer-2026-10-23): a chat's media as
    /// an album, opened from an attachment card or the Library grid.
    pub media_viewer: Option<MediaViewer>,
    /// The expanded chart overlay ([feedback chart-bubble-mac-2026-10-26]).
    pub chart_viewer: Option<crate::ui::chart_viewer::ChartViewer>,
    pub chart_viewer_generation: u64,
    /// Bumped on every open so the shell focuses the viewer once per opening.
    pub media_viewer_generation: u64,
    /// Bubble menu → Forward: the message the "Forward to" picker is open for.
    pub forward_message: Option<ForwardMessage>,
    pub new_project_open: bool,
    /// Connect an agent (#2141): the Create screen in its connect form is up.
    pub connect_open: bool,
    /// Connect an agent: the key reveal sheet, after the relay answered.
    pub connect_reveal: Option<ConnectReveal>,
    /// The Wake URL form of the connected agent whose pane is open.
    pub wake_form: Option<WakeForm>,
    /// Debug: per-message "Show details" disclosures in the transcript.
    pub show_details: bool,
    /// The user's one machine (projects are workspaces on it).
    pub machine: Machine,
    /// Settings → Connectors (F1).
    pub connectors: ConnectorsView,
    /// Settings → Secrets (D83).
    pub secrets: SecretsView,
    /// Settings → Devices (`GET /auth/devices`).
    pub devices: DevicesView,
    /// Agent profile → Secrets, per project.
    pub project_secrets: std::collections::HashMap<Id, ProjectSecrets>,
    /// Standing shell approvals per agent (#1544), loaded when the agent pane opens.
    pub shell_grants: std::collections::HashMap<Id, Vec<crate::model::ShellGrant>>,
    /// Secret hand-off request cards: approval id → saving / saved (env) / the error sentence.
    pub handoff: std::collections::HashMap<String, HandoffState>,
    /// Settings → Routines, per project (F3).
    pub routines: HashMap<Id, Vec<Routine>>,
    /// The box as `GET /machine/debug` last reported it — the agent pane's
    /// Computer tab ([feedback agent-pane-details-tab-2026-10-24]); fetched
    /// when an agent pane opens and when its Computer tab is picked.
    pub box_report: Option<BoxReport>,
    /// The agent's open background jobs per project, as the relay's activity
    /// record last listed them (duplex §2.4) — the tasks chip and drawer. A
    /// project with no entry or an empty list has no open job.
    pub jobs: HashMap<Id, Vec<Job>>,
    /// The tasks drawer is open over the presence row of this chat.
    pub tasks_open: Option<Id>,
    /// Each job's exchange as last fetched (`GET /projects/{id}/jobs/{jobId}`), by job id —
    /// the job tab's transcript is synthesised from it (`jobtabs::messages`).
    pub rollouts: HashMap<String, JobRollout>,
    /// Job tabs per agent project (`jobtabs`): one `job:<id>` chat per job the agent started,
    /// kept until the user closes it.
    pub job_tabs: HashMap<Id, Vec<Chat>>,
    /// Job tabs the user closed: not re-opened by the next listing (a link re-opens them).
    pub closed_job_tabs: HashSet<String>,
    /// Steers typed in a job tab the box has not echoed yet, by job id.
    pub pending_steers: HashMap<String, Vec<Message>>,
    /// Run history per routine, newest first.
    pub routine_runs: HashMap<Id, Vec<RoutineRun>>,
    /// System notifications (D49); the Shell flips `window_active`.
    pub notifier: Notifier,
    pub window_active: bool,
    /// One-time explainer before the OS asks for notification permission.
    pub notify_prompt: bool,
    /// Telegram-style profile sheet (user / group / agent) and its tab.
    pub profile: Option<ProfilePage>,
    pub profile_tab: usize,
    /// Pages under the open one, outermost first (a sender's profile pushed
    /// from a group, a member from its pane): `‹` pops back to the page with
    /// its tab and scroll.
    pub profile_stack: Vec<ProfileFrame>,
    /// Bumped whenever a different page fills the pane, so the shell starts
    /// its body at the top (a popped page keeps its saved scroll instead).
    pub profile_opened: u64,
    /// Agent page: the Appearance (tree + colour) section is unfolded.
    /// Rig / sidebar: open the avatar picker sheet for this entity on this tab (the shell
    /// drains it).
    pub avatar_picker_request: Option<(
        crate::ui::avatar_picker::Target,
        crate::ui::avatar_picker::Tab,
    )>,
    /// Rig: a prompt the shell types into the opened picker's Generate field and runs at once
    /// (`ARBOS_OPEN_PROFILE=me:avatar:generate=<prompt>`), so a shot can show prompt → result.
    pub avatar_rig_prompt: Option<String>,
    /// Rig: a file for the picker's Upload tab (`ARBOS_OPEN_PROFILE=me:avatar:upload=<path>`,
    /// also `agent:…`), applied as the file panel's pick so a shot shows the upload preview.
    pub avatar_rig_upload: Option<std::path::PathBuf>,
    /// The Generate tab's answer, for the shell's open picker.
    pub avatar_generated: Option<AvatarGenerated>,
    /// Agent page: which settings row (model / reasoning / voice) is dropped down.
    pub agent_pick: Option<AgentPick>,
    /// Agent page: the owner is editing the description inline.
    pub profile_editing_description: bool,
    /// Agent page: the owner is editing the name inline ("Edit" or a
    /// double-click on the name). Separate from `renaming_project` so the
    /// sidebar and header under the page do not grow a field of their own.
    pub profile_editing_name: bool,
    /// Double-click rename (sidebar / header): the project whose name is being
    /// edited inline.
    pub renaming_project: Option<Id>,
    /// Spaces whose notifications are muted (`<config>/muted.json`).
    pub muted: HashSet<Id>,
    /// Settings → Notifications: send and show Presence `{seen}` read receipts
    /// (`<config>/read_receipts.json`, default on).
    pub read_receipts: bool,
    /// Settings → Chat → Link previews (this Mac).
    pub link_previews: bool,
    /// People-chat read receipts: chat id → (peer handle → last seen message id).
    pub message_receipts: HashMap<Id, HashMap<String, Id>>,
    /// Last presence frame seen per handle (ms): "online" while fresh.
    pub presence_seen: HashMap<String, u64>,
    /// Answered this session: never re-arm it on a later sync, even when the
    /// marker file could not be written.
    notify_prompt_answered: bool,
    /// Last `FocusChat` sent to the source (dedupe).
    focus_sent: Option<Option<Id>>,
    /// Settings → Channels (F7).
    pub channels: ChannelsView,
    /// In-app update (D42): what the rail button and settings show.
    pub update: UpdateState,
    /// Automatic update installed; where the new app lives, until the safe-moment relaunch.
    pending_relaunch: Option<arbos_update::client::Install>,
    /// Last composer keystroke (any chat): the "never mid-typing" guard for the relaunch.
    last_input_at: u64,
    /// When the window last became focused. Auto-relaunch only counts time spent in
    /// Arbos (D85 "while you are in Arbos" / "when you pause"), so a Spaces swipe
    /// back after idle cannot be the restart edge.
    became_active_at: u64,
    /// A version the automatic install already failed on: offer it in the rail, do not loop.
    auto_update_failed: Option<String>,
    /// Settings → This Mac (F9).
    pub local: LocalView,
    /// F11: replies are spoken and a mic transcript sends at once.
    pub voice_mode: bool,
    /// Relay flag `features.voice_mode` (kill switch, default off): while off, no voice-mode
    /// UI (switch, picker, speaker rows, composer note) and no TTS request leaves the app.
    pub voice_mode_feature: bool,
    /// F11: assistant messages whose speech is cached (speaker icon replays).
    pub spoken: HashSet<Id>,
    /// F11: replies whose speech failed to arrive, decode or play, with the
    /// reason; the speaker row under that reply offers a retry. Scoped to the
    /// message, never shown as an app-wide banner.
    pub speech_failed: HashMap<Id, String>,
    pub awaiting_new_project: bool,
    /// Calls v1: live calls I know about per chat (the "On a call · Join" bar).
    pub calls_in_chat: HashMap<Id, Call>,
    /// Someone is calling me (ringing): the call and who started it.
    pub incoming_call: Option<(Call, Sender)>,
    /// When the ring card came up (auto-dismiss after `RING_TIMEOUT_MS`).
    incoming_since: Option<u64>,
    /// The ring (callee) and ringback (caller) tones while they should sound.
    ring_tone: Option<TonePlayer>,
    ringback_tone: Option<TonePlayer>,
    /// `GET /voices`: the call voices an agent can use (empty until fetched),
    /// and the id the relay uses when an agent has none set.
    pub voices: Vec<Voice>,
    pub default_voice: Option<String>,
    /// The voice sample playing now (url) and its player; cached paths by url.
    pub sample_playing: Option<String>,
    sample_player: Option<SamplePlayer>,
    sample_paths: HashMap<String, PathBuf>,
    /// A sample asked for before it was cached: play it when it lands.
    sample_wanted: Option<String>,
    voices_requested: bool,
    /// `call_missed` ids whose "Missed call from …" line this device wrote.
    missed_written: HashSet<String>,
    /// `call_missed` pushes that arrived before their chat was loaded (with when).
    missed_pending: Vec<(CallMissed, u64)>,
    /// Call ids whose "Call ended" line this device wrote (the leave answer and
    /// the `call_ended` push can both name this device as the writer).
    ended_written: HashSet<String>,
    /// The call this device is in.
    pub my_call: Option<ActiveCall>,
    /// D88: the relay runs servers (`GET /me/matrix` answered). False → no server rows,
    /// no "New server" / "Join a server" in the `+` sheet.
    pub servers_available: bool,
    /// D88: the servers I am in (`GET /servers`); `members` filled once a detail was fetched.
    pub servers: Vec<ServerInfo>,
    /// Rig: `ARBOS_OPEN_SERVER` was applied (once, when the registry first lists it).
    open_server_hook_done: bool,
    /// Rig: `ARBOS_OPEN_SERVER=…:menu|:menu-delete` — the shell opens the selected channel's
    /// row menu on its next render (`true` = with the Delete confirmation armed).
    pub rig_channel_menu: Option<bool>,
    /// Rig: `ARBOS_OPEN_PROFILE=group:member-menu[:confirm]` — the shell opens the first
    /// other member's row menu on the group pane (`true` = with the Remove confirm armed).
    pub rig_member_menu: Option<bool>,
    /// Rig: `ARBOS_OPEN_PROFILE=user:block:unblock` — unblock this person as soon as the
    /// block list names them (the row must read Unblock first, then Block user again).
    pub rig_unblock: Option<Person>,
    /// Rig: `ARBOS_OPEN_SPACE` applied once.
    pub rig_space_opened: bool,
    /// D88: the room list from the homeserver (previews, unread, `can_post`).
    pub matrix_snapshot: Snapshot,
    pub matrix_connected: bool,
    /// D88: decoded rows per channel room id; the transcript reads them like `messages`.
    pub channel_rows: HashMap<String, Vec<Message>>,
    /// D88: rooms that already got their second page of history (one extra page per session,
    /// asked for when the first page came back full).
    pub channel_paged: HashSet<String>,
    /// Servers whose member list was asked for this session (`want_server_rosters`).
    pub server_rosters_requested: HashSet<Id>,
    /// D88: the channel open in the centre (clears `selected_project` for the transcript).
    pub selected_channel: Option<ChannelSel>,
    /// D88: the server whose channel list the left pane slid into.
    pub open_server: Option<Id>,
    /// D88: the sheet open over the shell (new server / join / new channel).
    pub server_sheet: Option<ServerSheet>,
    /// D88: a server write is in flight (sheet buttons dim) and its last failure.
    pub server_busy: bool,
    pub server_error: Option<error_note::Slot>,
    /// A surface that is up and owns some verbs' failures (Settings → Secrets with a form
    /// open): a `Failure` for one of them is kept here for that surface instead of a toast.
    pub failure_claim: Option<FailureClaim>,
    pub claimed_failure: Option<error_note::Slot>,
    /// D88: a channel waiting for its room (`room_id` null right after creation).
    channel_pending_room: Option<(Id, Id)>,
    commands: mpsc::UnboundedSender<ClientCommand>,
    update_commands: mpsc::UnboundedSender<UpdateCommand>,
    call_commands: mpsc::UnboundedSender<CallCmd>,
    _pump: Task<()>,
    _notify_tick: Task<()>,
    _jobs_tick: Task<()>,
    _update_pump: Task<()>,
    _call_pump: Task<()>,
}

/// One tile in a call.
#[derive(Clone, Debug)]
pub struct CallParticipant {
    /// LiveKit identity (`u_<user_id>`); `local` for me.
    pub identity: String,
    pub sender: Sender,
    pub speaking: bool,
    pub muted: bool,
    pub video: Option<Arc<RenderImage>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallStatus {
    Connecting,
    Connected,
    Failed(String),
}

/// The call this device is in, as the UI shows it.
#[derive(Clone, Debug)]
pub struct ActiveCall {
    pub call: Call,
    pub status: CallStatus,
    pub muted: bool,
    pub camera: bool,
    pub participants: Vec<CallParticipant>,
    pub local_video: Option<Arc<RenderImage>>,
    pub joined_at: u64,
    /// The call strip at the top of the chat column (true) or the slim bar (false).
    pub expanded: bool,
    /// One line about a problem (no mic, no camera, reconnecting).
    pub notice: Option<String>,
    /// I started this call: ringback plays until someone else is in the room.
    pub outgoing: bool,
    /// From the 201 start response: "Robert's Arbos isn't open — they'll see a
    /// missed call" / "Ringing Robert's iPhone" while nobody has picked up
    /// (`None` = they are live).
    pub reachable: Option<String>,
    /// Voice API §11: the agent is sampling my camera right now ("<Agent> can see").
    pub agent_seeing: bool,
}

impl ActiveCall {
    pub fn participant_mut(&mut self, identity: &str) -> Option<&mut CallParticipant> {
        self.participants
            .iter_mut()
            .find(|p| p.identity == identity)
    }
}

/// Store `new` in `slot` and return the previous frame so the caller can
/// `App::drop_image` it (video frames upload into the window sprite atlas).
fn swap_frame(
    slot: &mut Option<Arc<RenderImage>>,
    new: Option<Arc<RenderImage>>,
) -> Option<Arc<RenderImage>> {
    std::mem::replace(slot, new)
}

/// Remove the participant with `identity` and return their last video frame.
fn remove_call_participant(active: &mut ActiveCall, identity: &str) -> Option<Arc<RenderImage>> {
    let idx = active
        .participants
        .iter()
        .position(|p| p.identity == identity)?;
    active.participants.remove(idx).video
}

fn drop_video_frame(cx: &mut App, frame: Option<Arc<RenderImage>>) {
    if let Some(image) = frame {
        cx.drop_image(image, None);
    }
}

fn drop_active_call_images(active: ActiveCall, cx: &mut App) {
    drop_video_frame(cx, active.local_video);
    for p in active.participants {
        drop_video_frame(cx, p.video);
    }
}

/// What the Details drawer (stub) describes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetailsTarget {
    Message { chat_id: Id, message_id: Id },
    Blob(String),
}

impl EventEmitter<AppEvent> for AppState {}

impl AppState {
    pub fn new(source: Box<dyn SyncSource>, cx: &mut Context<Self>) -> Self {
        let source_name = source.name();
        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<SyncEvent>();
        let (command_tx, command_rx) = mpsc::unbounded_channel::<ClientCommand>();

        let run = Tokio::spawn(cx, async move {
            if let Err(err) = source.run(event_tx, command_rx).await {
                eprintln!("sync source stopped: {err:#}");
            }
        });

        let pump = cx.spawn(async move |this, cx| {
            let _run = run;
            while let Some(event) = event_rx.recv().await {
                let applied = this.update(cx, |state, cx| state.apply(event, cx));
                if applied.is_err() {
                    break;
                }
            }
        });

        let notify_tick = cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(NOTIFY_TICK_MS))
                .await;
            if this.update(cx, |state, cx| state.notify_tick(cx)).is_err() {
                break;
            }
        });

        let jobs_tick = cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(JOBS_POLL_MS))
                .await;
            if this.update(cx, |state, _| state.jobs_tick()).is_err() {
                break;
            }
        });

        let mut update_state = UpdateState::new();
        // Rig: `ARBOS_UPDATE_STATE=available|downloading|installed|failed` —
        // the footer's update disc in that state for a fake 0.6.999 build
        // (the worker never polls while the variable is set).
        if let Some(phase) = std::env::var("ARBOS_UPDATE_STATE")
            .ok()
            .and_then(|v| UpdateState::fixture_phase(&v, &update_state.current_version))
        {
            update_state.phase = phase;
        }
        let (update_event_tx, mut update_event_rx) = mpsc::unbounded_channel::<UpdateEvent>();
        let (update_command_tx, update_command_rx) = mpsc::unbounded_channel::<UpdateCommand>();
        let update_worker = Tokio::spawn(
            cx,
            update::run(update_state.clone(), update_command_rx, update_event_tx),
        );
        let update_pump = cx.spawn(async move |this, cx| {
            let _worker = update_worker;
            while let Some(event) = update_event_rx.recv().await {
                let applied = this.update(cx, |state, cx| state.apply_update(event, cx));
                if applied.is_err() {
                    break;
                }
            }
        });
        let (call_event_tx, mut call_event_rx) = mpsc::unbounded_channel::<CallEvent>();
        let (call_command_tx, call_command_rx) = mpsc::unbounded_channel::<CallCmd>();
        let call_worker = Tokio::spawn(cx, calls::run(call_command_rx, call_event_tx));
        let call_pump = cx.spawn(async move |this, cx| {
            let _worker = call_worker;
            while let Some(event) = call_event_rx.recv().await {
                let applied = this.update(cx, |state, cx| state.apply_call_event(event, cx));
                if applied.is_err() {
                    break;
                }
            }
        });

        Self {
            source_name,
            pairing: None,
            account: None,
            mark: decode_image(include_bytes!(
                "../../../site/assets/logo/arbos-macos-icon-1024.png"
            ))
            .map(Arc::new),
            first_project_pending: false,
            onboarding: std::env::var("ARBOS_SHOW")
                .ok()
                .filter(|v| v == "onboarding")
                .map(|_| {
                    crate::onboarding::Onboarding::demo(
                        &std::env::var("ARBOS_ONBOARDING_STAGE").unwrap_or_default(),
                    )
                }),
            new_chat_open: false,
            new_group_open: false,
            new_group_draft: None,
            new_group_picked: Vec::new(),
            new_group_agents: Vec::new(),
            people_query: String::new(),
            people_results: Vec::new(),
            blocked: Vec::new(),
            people_busy: false,
            people_error: None,
            send_errors: HashMap::new(),
            people_directory: Vec::new(),
            typing: HashMap::new(),
            avatars: HashMap::new(),
            accents: HashMap::new(),
            awaiting_dm: None,
            draft_dm: None,
            pending_dm_reply: None,
            awaiting_group: None,
            invite_target: None,
            message_grant_target: None,
            add_members_target: None,
            add_members_server: None,
            add_members_secret: None,
            pending_join_code: None,
            avatar_reply_until: HashMap::new(),
            avatar_picker: None,
            profile_from_name: false,
            typing_sent_at: HashMap::new(),
            device_id: None,
            projects: Vec::new(),
            models: Vec::new(),
            project_docs: HashMap::new(),
            skills: Vec::new(),
            chats: HashMap::new(),
            messages: HashMap::new(),
            optimistic_notices: HashMap::new(),
            chat_ui: HashMap::new(),
            blobs: HashMap::new(),
            thumbnails: HashMap::new(),
            search_index: SearchIndex::default(),
            thumbnails_trimmed: HashMap::new(),
            thumbnails_requested: HashSet::new(),
            link_icons: HashMap::new(),
            link_icon_tones: HashMap::new(),
            link_icons_requested: HashSet::new(),
            thumbnails_failed: HashSet::new(),
            gif_results: None,
            gif_query_pending: None,
            sticker_packs: None,
            sticker_packs_requested: false,
            sticker_raw: HashMap::new(),
            copy_request: None,
            copy_request_seq: 0,
            notices: Vec::new(),
            push_refused: HashMap::new(),
            silence_until_armed: HashMap::new(),
            silence_until_gen: HashMap::new(),
            selected_project: None,
            store_usage: (0, 0),
            active_tab: HashMap::new(),
            multi_chat: multi_chat_enabled(),
            last_chats: HashMap::new(),
            archived_chats: HashSet::new(),
            pins: Vec::new(),
            pin_cap: PIN_CAP_DEFAULT,
            search_chat: None,
            reveal_message: None,
            reveal_nonce: 0,
            highlight_message: None,
            chat_search: None,
            link: Link::Connecting,
            room_links: HashMap::new(),
            relay_link: (LinkState::Live, 0),
            rail_collapsed: crate::ui::theme::SidebarCollapsed::load(),
            sidebar_width: crate::ui::theme::SidebarWidth::load(),
            // Rig: `ARBOS_CALL_STRIP_HEIGHT=<pt>` starts the strip at that height (the
            // resized state, for screenshots without a pointer).
            call_strip_height: std::env::var("ARBOS_CALL_STRIP_HEIGHT")
                .ok()
                .and_then(|v| v.trim().parse::<f32>().ok())
                .map(|v| crate::ui::callview::strip_layout::clamp_height(v, 780.0))
                .unwrap_or(crate::ui::callview::strip_layout::DEFAULT_HEIGHT),
            sidebar_edge_drag: crate::ui::sidebar_fit::EdgeDrag::default(),
            sidebar_overlay: false,
            details_open: None,
            media_viewer: None,
            media_viewer_generation: 0,
            chart_viewer: None,
            chart_viewer_generation: 0,
            forward_message: None,
            new_project_open: false,
            connect_open: false,
            connect_reveal: None,
            wake_form: None,
            show_details: false,
            machine: Machine::default(),
            connectors: ConnectorsView::default(),
            secrets: SecretsView::default(),
            devices: DevicesView::default(),
            project_secrets: std::collections::HashMap::new(),
            shell_grants: std::collections::HashMap::new(),
            handoff: std::collections::HashMap::new(),
            routines: HashMap::new(),
            box_report: None,
            jobs: HashMap::new(),
            tasks_open: None,
            rollouts: HashMap::new(),
            job_tabs: HashMap::new(),
            closed_job_tabs: HashSet::new(),
            pending_steers: HashMap::new(),
            routine_runs: HashMap::new(),
            notifier: Notifier::new(),
            window_active: true,
            notify_prompt: false,
            profile: None,
            profile_tab: 0,
            profile_stack: Vec::new(),
            profile_opened: 0,
            avatar_picker_request: None,
            avatar_rig_prompt: None,
            avatar_rig_upload: None,
            avatar_generated: None,
            agent_pick: None,
            profile_editing_description: false,
            profile_editing_name: false,
            renaming_project: None,
            muted: load_muted(),
            read_receipts: load_read_receipts(),
            link_previews: {
                let on = load_link_previews();
                crate::ui::link_card::set_enabled(on);
                on
            },
            message_receipts: HashMap::new(),
            presence_seen: HashMap::new(),
            notify_prompt_answered: false,
            focus_sent: None,
            channels: ChannelsView::default(),
            update: update_state,
            pending_relaunch: None,
            last_input_at: 0,
            became_active_at: now_ms(),
            auto_update_failed: None,
            local: LocalView::default(),
            voice_mode: false,
            voice_mode_feature: false,
            spoken: HashSet::new(),
            speech_failed: HashMap::new(),
            awaiting_new_project: false,
            commands: command_tx,
            update_commands: update_command_tx,
            call_commands: call_command_tx,
            calls_in_chat: HashMap::new(),
            incoming_call: None,
            incoming_since: None,
            ring_tone: None,
            ringback_tone: None,
            voices: Vec::new(),
            default_voice: None,
            sample_playing: None,
            sample_player: None,
            sample_paths: HashMap::new(),
            sample_wanted: None,
            voices_requested: false,
            missed_written: HashSet::new(),
            missed_pending: Vec::new(),
            ended_written: HashSet::new(),
            my_call: None,
            servers_available: false,
            open_server_hook_done: false,
            rig_channel_menu: None,
            rig_member_menu: None,
            rig_unblock: None,
            rig_space_opened: false,
            servers: Vec::new(),
            matrix_snapshot: Snapshot::default(),
            matrix_connected: false,
            channel_rows: HashMap::new(),
            channel_paged: HashSet::new(),
            server_rosters_requested: HashSet::new(),
            selected_channel: None,
            open_server: None,
            server_sheet: None,
            server_busy: false,
            server_error: None,
            failure_claim: None,
            claimed_failure: None,
            channel_pending_room: None,
            _pump: pump,
            _notify_tick: notify_tick,
            _jobs_tick: jobs_tick,
            _update_pump: update_pump,
            _call_pump: call_pump,
        }
    }

    fn apply_update(&mut self, event: UpdateEvent, cx: &mut Context<Self>) {
        match event {
            UpdateEvent::Checking => {
                if matches!(self.update.phase, UpdatePhase::Idle | UpdatePhase::UpToDate) {
                    self.update.phase = UpdatePhase::Checking;
                }
            }
            UpdateEvent::UpToDate => {
                self.update.last_checked_at = Some(now_ms());
                if !self.update.busy() && self.pending_relaunch.is_none() {
                    self.update.phase = UpdatePhase::UpToDate;
                }
            }
            UpdateEvent::Available(available) => {
                let available = *available;
                self.update.last_checked_at = Some(now_ms());
                if self.pending_relaunch.is_some() {
                    self.supersede_pending_install(available, cx);
                } else if !self.update.busy() {
                    // Never offer a build that is not strictly newer than what is running.
                    if !arbos_update::version::is_newer(
                        &available.item.version,
                        &self.update.current_version,
                    ) {
                        self.update.phase = UpdatePhase::UpToDate;
                    } else {
                        let fresh = self.update.available() != Some(&available);
                        let version = available.item.version.clone();
                        self.update.phase = UpdatePhase::Available(available);
                        let auto = self.update.auto
                            && !self.update.needs_relocation
                            && self.auto_update_failed.as_deref() != Some(version.as_str());
                        if auto {
                            // D85: download, verify and install in the background; the
                            // relaunch waits for a safe moment.
                            update::log(format!(
                                "auto-update: installing {version} in the background"
                            ));
                            self.install_update(cx);
                        } else if fresh {
                            self.push_notice(format!(
                                "Arbos {version} is ready — press the blue update button at the bottom of the chat list to install it."
                            ));
                        }
                    }
                }
            }
            UpdateEvent::Progress { got, total } => {
                if let Some(available) = self.update.available().cloned() {
                    self.update.phase = UpdatePhase::Downloading {
                        available,
                        got,
                        total,
                    };
                }
            }
            UpdateEvent::Installing => {
                if let Some(available) = self.update.available().cloned() {
                    self.update.phase = UpdatePhase::Installing(available);
                }
            }
            UpdateEvent::Installed(installed) => {
                if self.update.needs_relocation {
                    self.update.needs_relocation = false;
                    self.push_notice(format!(
                        "Arbos now lives in {}; the copy you opened it from can be deleted.",
                        installed
                            .path()
                            .parent()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "Applications".into())
                    ));
                    self.relaunch_into(installed, cx);
                    return;
                }
                let Some(available) = self.update.available().cloned() else {
                    self.relaunch_into(installed, cx);
                    return;
                };
                if !self.update.auto {
                    // Manual path (toggle off): the click on Update meant "now".
                    self.relaunch_into(installed, cx);
                    return;
                }
                // Automatic path: relaunch when it is safe, at the latest after the countdown
                // (unless a call or typing is in the way).
                let version = available.item.version.clone();
                self.update.phase = UpdatePhase::Installed {
                    available,
                    deadline_ms: now_ms() + update::RELAUNCH_COUNTDOWN_MS,
                };
                // A newer build installed over one that was already waiting keeps the watcher
                // that is running for it.
                let watching = self.pending_relaunch.replace(installed).is_some();
                update::log(format!(
                    "auto-update: {version} installed; waiting for a safe moment to relaunch"
                ));
                if self.try_relaunch(cx) {
                    return;
                }
                self.push_notice(format!(
                    "Arbos {version} is installed — it restarts in a minute while you are in Arbos, \
                     or when you pause. Press the blue update button at the bottom of the chat list to do it at once."
                ));
                if !watching {
                    cx.spawn(async move |this, cx| loop {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(1000))
                            .await;
                        let done = this.update(cx, |state, cx| {
                            if state.pending_relaunch.is_none() {
                                return true;
                            }
                            let relaunched = state.try_relaunch(cx);
                            cx.notify();
                            relaunched
                        });
                        if done.unwrap_or(true) {
                            break;
                        }
                    })
                    .detach();
                }
            }
            UpdateEvent::Failed(message) => {
                if let Some(a) = self.update.available() {
                    if self.update.auto {
                        self.auto_update_failed = Some(a.item.version.clone());
                    }
                }
                // The disc alone is not enough: say what went wrong and offer the download page.
                let next = if message.ends_with('.') {
                    " Press the blue update button to retry"
                } else {
                    " — press the blue update button to retry"
                };
                self.push_notice(format!(
                    "Update failed: {message}{next}, or get the newest version from {}.",
                    update::DOWNLOAD_URL
                ));
                self.update.phase = UpdatePhase::Failed {
                    available: self.update.available().cloned(),
                    message,
                };
            }
        }
        cx.notify();
    }

    /// Is this a moment the app may restart itself? Never in a call, right after a keystroke, or
    /// while another app has focus (relaunch would steal it). When Arbos is focused: after a pause
    /// with no typing, or when the countdown is over — both measured only for continuous focus,
    /// so swiping back to an idle window is not the restart.
    fn relaunch_is_safe(&self) -> bool {
        let now = now_ms();
        if self.my_call.is_some() || self.incoming_call.is_some() {
            return false;
        }
        // A newer build is being swapped into the bundle the relaunch would open.
        if self.update.busy() {
            return false;
        }
        if !self.window_active {
            return false;
        }
        if now.saturating_sub(self.last_input_at) < update::TYPING_GUARD_MS {
            return false;
        }
        let focused_for = now.saturating_sub(self.became_active_at);
        let countdown_over = match &self.update.phase {
            UpdatePhase::Installed { deadline_ms, .. } => {
                // Wall deadline alone used to fire on the focus-regain edge after time away.
                // Require a full minute of continuous focus too ("while you are in Arbos").
                now >= *deadline_ms && focused_for >= update::RELAUNCH_COUNTDOWN_MS
            }
            UpdatePhase::Idle
            | UpdatePhase::Checking
            | UpdatePhase::UpToDate
            | UpdatePhase::Available(_)
            | UpdatePhase::Downloading { .. }
            | UpdatePhase::Installing(_)
            | UpdatePhase::Restarting(_)
            | UpdatePhase::Failed { .. } => focused_for >= update::RELAUNCH_COUNTDOWN_MS,
        };
        let paused = now.saturating_sub(self.last_input_at) >= update::IDLE_RELAUNCH_MS
            && focused_for >= update::IDLE_RELAUNCH_MS;
        paused || countdown_over
    }

    /// Relaunch into the pending install when it is safe; true when the relaunch was started.
    fn try_relaunch(&mut self, cx: &mut Context<Self>) -> bool {
        if self.pending_relaunch.is_none() || !self.relaunch_is_safe() {
            return false;
        }
        let Some(installed) = self.pending_relaunch.take() else {
            return false;
        };
        update::log("auto-update: safe moment — relaunching");
        self.relaunch_into(installed, cx);
        true
    }

    /// A newer build announced while an older one waits for its relaunch (the app sat
    /// unfocused while releases landed) is installed over it first, so the user gets one
    /// restart into the newest build instead of one restart per release they missed.
    /// Manual mode keeps the waiting build: the user already chose to restart into it.
    fn supersede_pending_install(
        &mut self,
        available: arbos_update::client::Available,
        cx: &mut Context<Self>,
    ) {
        let UpdatePhase::Installed {
            available: waiting, ..
        } = &self.update.phase
        else {
            return;
        };
        let version = available.item.version.clone();
        if !self.update.auto
            || self.auto_update_failed.as_deref() == Some(version.as_str())
            || !arbos_update::version::is_newer(&version, &waiting.item.version)
        {
            return;
        }
        update::log(format!(
            "auto-update: {version} supersedes the waiting {}; installing it before the relaunch",
            waiting.item.version
        ));
        self.update.phase = UpdatePhase::Downloading {
            available,
            got: 0,
            total: None,
        };
        let _ = self.update_commands.send(UpdateCommand::Install);
        cx.notify();
    }

    /// One frame shows "Restarting", then the relauncher takes over.
    fn relaunch_into(&mut self, installed: arbos_update::client::Install, cx: &mut Context<Self>) {
        if let Some(available) = self.update.available().cloned() {
            self.update.phase = UpdatePhase::Restarting(available);
        }
        self.pending_relaunch = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(700))
                .await;
            let _ = this.update(cx, |state, cx| match update::relaunch(&installed) {
                Ok(()) => cx.quit(),
                Err(err) => {
                    update::log(format!("relaunch failed: {err:#}"));
                    state.push_notice(
                        "Arbos was updated but could not relaunch itself — quit and open Arbos again.",
                    );
                    state.update.phase = UpdatePhase::Failed {
                        message: format!("installed, but could not relaunch: {err:#}"),
                        available: state.update.available().cloned(),
                    };
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Settings → Updates → Automatic updates.
    pub fn set_auto_update(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.update.auto != enabled {
            self.update.auto = enabled;
            update::save_auto_update(enabled);
            update::log(format!(
                "auto-update: {}",
                if enabled { "on" } else { "off" }
            ));
            if enabled && matches!(self.update.phase, UpdatePhase::Available(_)) {
                self.install_update(cx);
            }
        }
        cx.notify();
    }

    pub fn check_for_updates(&mut self, cx: &mut Context<Self>) {
        if self.update.busy() {
            return;
        }
        let _ = self.update_commands.send(UpdateCommand::Check);
        cx.notify();
    }

    /// The failure notice's fallback: open arbos.life/download/mac.
    pub fn open_download_page(&mut self, cx: &mut Context<Self>) {
        update::open_download_page();
        cx.notify();
    }

    /// "Move to Applications" (translocated / read-only copy): copy, clear quarantine, relaunch.
    pub fn move_to_applications(&mut self, cx: &mut Context<Self>) {
        if self.update.busy() {
            return;
        }
        let _ = self.update_commands.send(UpdateCommand::MoveToApplications);
        cx.notify();
    }

    /// The footer's update disc (and Settings → Updates → Install): one click does what
    /// [`UpdateState::disc_action`] says — download, verify, swap and relaunch; restart now
    /// for an installed build; from a translocated or read-only copy the move to Applications.
    pub fn install_update(&mut self, cx: &mut Context<Self>) {
        if self.update.busy() {
            return;
        }
        match self.update.click_action() {
            update::DiscAction::Hidden | update::DiscAction::Busy => {}
            update::DiscAction::Relaunch => {
                // "Restart now": the automatic install is done; skip the wait.
                if let Some(installed) = self.pending_relaunch.take() {
                    update::log("auto-update: Restart now pressed");
                    self.relaunch_into(installed, cx);
                }
            }
            update::DiscAction::MoveToApplications => self.move_to_applications(cx),
            update::DiscAction::Install => {
                let Some(available) = self.update.available().cloned() else {
                    return;
                };
                self.update.phase = UpdatePhase::Downloading {
                    available,
                    got: 0,
                    total: None,
                };
                let _ = self.update_commands.send(UpdateCommand::Install);
                cx.notify();
            }
        }
    }

    /// Headless tests: swap the source's command channel for one the test reads,
    /// so what the UI sent (`stop { jobId }`, …) can be asserted.
    #[cfg(test)]
    pub(crate) fn take_command_channel(&mut self) -> mpsc::UnboundedReceiver<ClientCommand> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.commands = tx;
        rx
    }

    /// One sync event into the state. A panic while applying it (a row shape the code did
    /// not expect: 2026-10-26, Mac 0.6.268 crashed at every launch restoring the saved
    /// chats) is caught here, logged by the panic hook (`panic_log`) and the event is skipped
    /// — the app stays up with that one row missing instead of aborting on the main thread.
    pub(crate) fn apply(&mut self, event: SyncEvent, cx: &mut Context<Self>) {
        let label = sync_event_label(&event);
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.apply_inner(event, cx)));
        if result.is_err() {
            log::error!(target: "arbos_mac::diagnostic", "sync event skipped after a panic: {label}");
            self.push_notice(
                "One update could not be shown; it was skipped. The details are in the log."
                    .to_owned(),
            );
            cx.notify();
        }
    }

    fn apply_inner(&mut self, event: SyncEvent, cx: &mut Context<Self>) {
        #[cfg(test)]
        if let SyncEvent::Notice(text) = &event {
            // The restore guard's regression test: a notice that reads "panic!" is the
            // stand-in for a row shape that panics deep inside an apply.
            assert!(text != "panic!", "test poison event");
        }
        match event {
            SyncEvent::OutsideAgentKey {
                project_id,
                name,
                transport,
                key,
                link,
                relay_url,
                chat_id,
                rotated,
            } => {
                self.awaiting_new_project = false;
                self.connect_reveal = Some(ConnectReveal {
                    project_id: project_id.clone(),
                    name,
                    transport,
                    key,
                    link,
                    relay_url,
                    chat_id,
                    rotated,
                    status: None,
                    copied: None,
                });
                self.poll_connect_status(cx);
                cx.notify();
            }
            SyncEvent::OutsideAgentStatus { project_id, info } => {
                if let Some(r) = self
                    .connect_reveal
                    .as_mut()
                    .filter(|r| r.project_id == project_id)
                {
                    r.status = Some(info.clone());
                }
                if let Some(p) = self.projects.iter_mut().find(|p| p.id == project_id) {
                    p.outside = Some(info);
                }
                cx.notify();
            }
            SyncEvent::OutsideAgentWakeSaved { project_id, error } => {
                let form = self.wake_form_mut(&project_id);
                form.saving = false;
                form.note = Some(match error {
                    None => (true, "Saved".to_owned()),
                    Some(e) => (false, e),
                });
                cx.notify();
            }
            SyncEvent::OutsideAgentWakeTested {
                project_id,
                ok,
                text,
            } => {
                let form = self.wake_form_mut(&project_id);
                form.sending = false;
                form.note = Some((ok, text));
                cx.notify();
            }
            SyncEvent::NeedsPairing { relay_url } => {
                let mode = self
                    .pairing
                    .as_ref()
                    .map(|p| p.mode)
                    .unwrap_or(SignInMode::GitHub);
                self.pairing = Some(Pairing {
                    relay_url,
                    mode,
                    busy: false,
                    error: None,
                    error_details_open: false,
                    flow: sign_in::Flow::default(),
                });
                self.device_id = None;
                self.room_links.clear();
                self.push_refused.clear();
                self.relay_link = (LinkState::Live, 0);
                self.people_directory.clear();
                cx.notify();
            }
            SyncEvent::Paired { device_id } => {
                self.pairing = None;
                self.device_id = Some(device_id);
                self.maybe_prompt_notifications();
                // Land in a chat: if this account has no project yet, make "Home".
                self.first_project_pending = true;
                self.begin_onboarding(cx);
                cx.notify();
            }
            SyncEvent::Features { voice_mode } => {
                self.voice_mode_feature = voice_mode;
                if !voice_mode && self.voice_mode {
                    self.set_voice_mode(false, cx);
                }
                if let Some(chat_id) = self.active_chat_id() {
                    cx.emit(AppEvent::TranscriptChanged { chat_id });
                }
                cx.notify();
            }
            SyncEvent::Account(account) => {
                self.account = account;
                if self.account.is_some() {
                    self.maybe_prompt_notifications();
                    self.prefetch_directory();
                    // Align the sync source with the saved Settings → Notifications
                    // preference (Shared defaults to on); no PUT on every launch.
                    let enabled = self.read_receipts;
                    self.command(ClientCommand::SetReadReceipts {
                        enabled,
                        persist: false,
                    });
                }
                if let Some(url) = self.account.as_ref().map(|a| a.avatar_url.clone()) {
                    self.want_avatar(url);
                }
                cx.notify();
            }
            SyncEvent::PeopleSearch { query, results } => {
                let urls: Vec<String> = results.iter().map(|p| p.avatar_url.clone()).collect();
                for url in urls {
                    self.want_avatar(url);
                }
                if is_directory_query(&query) {
                    self.people_directory = results.clone();
                }
                if query == self.people_query {
                    let fresh = people_sheet_results(&self.people_directory, &results, &query);
                    if let Some(changed) = directory_update(&self.people_results, fresh) {
                        self.people_results = changed;
                    }
                    self.people_busy = false;
                } else if is_directory_query(&query) && !is_directory_query(&self.people_query) {
                    // The directory landed after the user started typing: re-match it.
                    let fresh = people_sheet_results(
                        &self.people_directory,
                        &self.people_results,
                        &self.people_query,
                    );
                    if let Some(changed) = directory_update(&self.people_results, fresh) {
                        self.people_results = changed;
                    }
                }
                cx.notify();
            }
            SyncEvent::CallReady {
                call,
                created,
                reachable,
            } => self.on_call_ready(call, created, reachable, cx),
            SyncEvent::CallStarted { call, sender } => self.on_call_started(call, sender, cx),
            SyncEvent::ChannelMessage {
                server_id,
                server_name,
                channel_id,
                channel_name,
                room_id,
                sender,
                text,
            } => self.notify_channel_message(
                &server_id,
                &server_name,
                &channel_id,
                &channel_name,
                &room_id,
                &sender,
                text.as_deref(),
            ),
            SyncEvent::FeedbackCapture {
                request_id,
                expires_at,
            } => {
                let screen = self
                    .selected()
                    .map(|p| format!("project:{}", p.name))
                    .unwrap_or_else(|| "home".into());
                if crate::ui::feedback::answer_capture(request_id, expires_at, screen, cx) {
                    self.push_notice("Screenshot added to your feedback");
                    cx.notify();
                }
            }
            SyncEvent::CallEnded(ended) => self.on_call_ended(ended, cx),
            SyncEvent::CallMissed(missed) => self.on_call_missed(missed, cx),
            SyncEvent::CallInfo(call) => self.on_call_info(call, cx),
            SyncEvent::CallSeen { chat_id, call_id } => {
                let space = self.space_of_chat(&chat_id);
                let known = self
                    .calls_in_chat
                    .get(&space)
                    .map(|c| c.call_id == call_id)
                    .unwrap_or(false);
                if !known {
                    self.command(ClientCommand::FetchCall { call_id });
                }
            }
            SyncEvent::CallError { chat_id, error } => {
                let _ = chat_id;
                if matches!(
                    self.my_call.as_ref().map(|c| &c.status),
                    Some(CallStatus::Connecting)
                ) {
                    if let Some(active) = self.my_call.take() {
                        drop_active_call_images(active, cx);
                    }
                }
                self.push_failure(error, None);
                cx.notify();
            }
            SyncEvent::Blocked(people) => {
                self.blocked = people;
                if let Some(person) = self
                    .rig_unblock
                    .take_if(|p| blocked_entry(&self.blocked, p).is_some())
                {
                    self.unblock_user(&person, cx);
                }
                cx.notify();
            }
            SyncEvent::PeopleError(err) => {
                self.people_busy = false;
                self.withdraw_optimistic_notices(cx);
                // The New Message sheet shows the note inline; a DM started
                // from a person's pane (its Message pill) has no sheet, so the
                // relay's words ("this person is not available") go to a toast.
                if !self.new_chat_open && self.awaiting_dm.is_some() {
                    self.push_failure(err.clone(), None);
                }
                self.fail_draft_dm(&err, cx);
                self.people_error = Some(error_note::Slot::new(err));
                self.awaiting_dm = None;
                self.awaiting_group = None;
                cx.notify();
            }
            SyncEvent::PeopleGroupCreated { chat_id } => {
                // Comes after the Projects event that lists the group, so the
                // sidebar row exists; a stale id (list not refreshed) is left to
                // the members match below.
                if self.awaiting_group.is_some() && self.project(&chat_id).is_some() {
                    self.group_created(chat_id, cx);
                }
            }
            SyncEvent::Presence {
                handle,
                chat_id,
                seen,
            } => {
                self.presence_seen.insert(handle.clone(), now_ms());
                if let (Some(chat_id), Some(seen)) = (chat_id, seen) {
                    self.message_receipts
                        .entry(chat_id)
                        .or_default()
                        .insert(handle, seen);
                    cx.notify();
                } else if self.profile.is_some() {
                    cx.notify();
                }
            }
            SyncEvent::Typing {
                chat_id,
                handle,
                active,
            } => {
                let entry = self.typing.entry(chat_id).or_default();
                entry.retain(|(h, _)| *h != handle);
                if active {
                    entry.push((handle, now_ms() + 6_000));
                }
                cx.notify();
            }
            SyncEvent::ImageGenerated {
                target,
                image_key,
                url,
                error,
            } => {
                if let Some(u) = url.clone() {
                    self.want_avatar(u);
                }
                self.avatar_generated = Some(AvatarGenerated {
                    target,
                    image_key,
                    url,
                    error,
                });
                cx.notify();
            }
            SyncEvent::Avatar { url, bytes } => {
                // One decode serves both the picture and the bubble-bar colour.
                let rgba = decode_rgba(&bytes);
                if let Some(accent) = rgba.as_ref().and_then(|img| {
                    let (w, h) = img.dimensions();
                    arbos_core::avatar::dominant_accent(img.as_raw(), w, h)
                }) {
                    self.accents.insert(url.clone(), accent);
                }
                let image = rgba.and_then(bgra_frame).map(Arc::new);
                self.avatars.insert(url, image);
                cx.notify();
            }
            SyncEvent::AvatarMissing { url } => {
                // Not "no picture" for good: a later `want_avatar` (the next registry event,
                // a reconnect) requests it again instead of drawing the placeholder forever.
                self.avatars.remove(&url);
                cx.notify();
            }
            SyncEvent::PairingFailed(err) => {
                if let Some(p) = &mut self.pairing {
                    p.busy = false;
                    p.error = Some(err.clone());
                    let attempt = p.flow.attempt;
                    p.flow.failed(attempt, err);
                }
                cx.notify();
            }
            SyncEvent::ChatCreated {
                project_id,
                local_id,
                chat,
            } => {
                let tabs = self.chats.entry(project_id.clone()).or_default();
                match tabs.iter_mut().find(|c| c.id == local_id) {
                    Some(slot) => *slot = chat.clone(),
                    None => tabs.push(chat.clone()),
                }
                if let Some(msgs) = self.messages.remove(&local_id) {
                    self.messages.insert(chat.id.clone(), msgs);
                    self.search_index.remove_chat(&local_id);
                    self.index_chat(&chat.id.clone());
                }
                if let Some(ui) = self.chat_ui.remove(&local_id) {
                    self.chat_ui.insert(chat.id.clone(), ui);
                }
                if self.active_tab.get(&project_id) == Some(&local_id) {
                    self.active_tab.insert(project_id, chat.id.clone());
                    self.command(ClientCommand::OpenChat {
                        chat_id: chat.id.clone(),
                    });
                }
                cx.emit(AppEvent::TranscriptChanged { chat_id: chat.id });
                cx.notify();
            }
            SyncEvent::ProjectDoc { project_id, view } => {
                self.project_docs.insert(project_id, view);
                cx.notify();
            }
            SyncEvent::Blob {
                project_id,
                key,
                mime,
                bytes,
                saved_to,
            } => {
                if self.selected_project.as_deref() != Some(project_id.as_str()) {
                    return;
                }
                if let Some(path) = &saved_to {
                    self.push_notice(format!("Saved to {}", path.display()));
                }
                self.blobs.insert(
                    key.clone(),
                    BlobView {
                        mime,
                        bytes,
                        saved_to,
                    },
                );
                self.details_open = Some(DetailsTarget::Blob(key));
                cx.notify();
            }
            SyncEvent::LinkIcon { host, png } => {
                let rgba = png.and_then(|b| decode_rgba(&b));
                let tone = rgba
                    .as_ref()
                    .map(crate::ui::link_tile::tone_of)
                    .unwrap_or_default();
                let image = rgba.and_then(bgra_frame).map(Arc::new);
                self.link_icon_tones.insert(host.clone(), tone);
                self.link_icons.insert(host, image);
                cx.notify();
            }
            SyncEvent::Thumbnail { key, bytes } => {
                if key.starts_with(crate::model::STICKER_KEY_PREFIX) {
                    self.sticker_raw.insert(key.clone(), bytes.clone());
                }
                if let Some((image, trimmed)) = decode_image_pair(&bytes) {
                    self.thumbnails_failed.remove(&key);
                    match trimmed {
                        Some(t) => {
                            self.thumbnails_trimmed.insert(key.clone(), Arc::new(t));
                        }
                        None => {
                            self.thumbnails_trimmed.remove(&key);
                        }
                    }
                    self.thumbnails.insert(key, Arc::new(image));
                    if let Some(chat_id) = self.active_chat_id() {
                        cx.emit(AppEvent::TranscriptChanged { chat_id });
                    }
                    cx.notify();
                } else {
                    // Bytes arrived but could not be decoded (unsupported format,
                    // corrupt file, …): show "No preview", not Loading forever.
                    self.thumbnails_failed.insert(key);
                    if let Some(chat_id) = self.active_chat_id() {
                        cx.emit(AppEvent::TranscriptChanged { chat_id });
                    }
                    cx.notify();
                }
            }
            SyncEvent::ThumbnailFailed { key } => {
                self.thumbnails_failed.insert(key);
                if let Some(chat_id) = self.active_chat_id() {
                    cx.emit(AppEvent::TranscriptChanged { chat_id });
                }
                cx.notify();
            }
            SyncEvent::GifResults { query, pos, page } => {
                self.gif_results_arrived(query, pos, page);
                cx.notify();
            }
            SyncEvent::StickerPacks { packs } => {
                self.sticker_packs = Some(packs);
                cx.notify();
            }
            SyncEvent::CopyBlobs { request, blobs } => {
                self.finish_copy(request, blobs, cx);
            }
            SyncEvent::AttachmentSaved { path } => {
                self.push_notice(format!("Saved to {}", path.display()));
                crate::ui::attachments::reveal(&path);
                cx.notify();
            }
            SyncEvent::AttachmentOpened { key, path } => {
                if let Some(viewer) = self.media_viewer.as_mut() {
                    if viewer.busy.as_ref() == Some(&key) {
                        viewer.busy = None;
                        // Video Play on macOS: keep the temp file and show it
                        // inline (AVPlayerView) instead of handing off to QuickTime.
                        if let Some(path) = path.filter(|_| {
                            viewer
                                .current()
                                .is_some_and(|i| i.attachment.kind() == AttachmentKind::Video)
                        }) {
                            viewer.playing = Some(path);
                        }
                    }
                }
                cx.notify();
            }
            SyncEvent::SecretHandoff {
                approval_id,
                result,
            } => {
                self.handoff.insert(
                    approval_id,
                    match result {
                        Ok(env) => HandoffState::Saved(env),
                        Err(e) => HandoffState::Failed(crate::ui::error_note::Slot::new(e)),
                    },
                );
                cx.notify();
            }
            SyncEvent::Notice(text) => {
                self.push_notice(text);
                cx.notify();
            }
            SyncEvent::Failure { error, retry } => {
                // Create failed: stop waiting for a project that will not arrive.
                if error.verb == "create the agent" {
                    self.awaiting_new_project = false;
                }
                let claimed = self
                    .failure_claim
                    .as_ref()
                    .is_some_and(|c| c.verbs.iter().any(|v| error.verb.starts_with(v)));
                if claimed {
                    self.claimed_failure = Some(error_note::Slot::new(error));
                } else {
                    self.push_failure(error, retry);
                }
                cx.notify();
            }
            SyncEvent::PushRefused {
                chat_id,
                refused,
                why,
            } => {
                const DELIVERY: &str = "Messages in this chat are not being delivered";
                if refused {
                    // The chat-level note draws `why` until the room's next ACK.
                    self.push_refused.insert(chat_id, why);
                    self.arm_silence_end_timers(cx);
                } else {
                    self.push_refused.remove(&chat_id);
                    self.notices.retain(|n| n.text != DELIVERY);
                }
                cx.notify();
            }
            SyncEvent::Models(models) => {
                self.models = models;
                cx.notify();
            }
            SyncEvent::Projects(projects) => {
                // Signed in and synced (also the path without a sign-in event).
                if !projects.is_empty() {
                    self.maybe_prompt_notifications();
                    // The agent pane's "Voice · <name>" row needs the list once.
                    if !self.voices_requested {
                        self.voices_requested = true;
                        self.fetch_voices();
                    }
                }
                for p in &projects {
                    let crossed = p.spend_cap_usd > 0.0 && p.spend_today_usd >= p.spend_cap_usd;
                    let was_below = self
                        .project(&p.id)
                        .map(|old| old.spend_today_usd < old.spend_cap_usd)
                        .unwrap_or(false);
                    if crossed && was_below {
                        self.notify(
                            p.id.clone(),
                            None,
                            NotifyKind::SpendCapReached,
                            format!("${:.2} of ${:.2} today", p.spend_today_usd, p.spend_cap_usd),
                        );
                    }
                }
                for p in &projects {
                    for person in &p.people {
                        let url = person.avatar_url.clone();
                        self.want_avatar(url);
                    }
                    if let Some(url) = p.image_url.clone() {
                        self.want_avatar(url);
                    }
                    for url in p.agents.iter().filter_map(|a| a.image_url.clone()) {
                        self.want_avatar(url);
                    }
                }
                if let Some(handle) = self.awaiting_dm.clone() {
                    if let Some(space) = projects.iter().find(|p| {
                        p.kind == SpaceKind::Dm && p.people.iter().any(|x| x.handle == handle)
                    }) {
                        self.awaiting_dm = None;
                        self.new_chat_open = false;
                        let id = space.id.clone();
                        self.projects = projects.clone();
                        self.select_project(id.clone(), cx);
                        self.land_draft_dm(&handle, &id, cx);
                        self.arm_pending_dm_reply(cx);
                    }
                }
                // Members match (older relays / the fixture before it sends
                // `PeopleGroupCreated`): a NEW people group with exactly the
                // picked handles. Agents-only picks have no handles to match on
                // and wait for the id event instead.
                if let Some(handles) = self.awaiting_group.clone() {
                    if let Some(id) = created_group(&self.projects, &projects, &handles) {
                        self.projects = projects.clone();
                        self.group_created(id, cx);
                    }
                }
                if self.first_project_pending {
                    self.first_project_pending = false;
                    if projects.is_empty() && !self.awaiting_new_project {
                        self.awaiting_new_project = true;
                        self.command(ClientCommand::CreateProject {
                            name: "Home".into(),
                            avatar_color_index: None,
                            leaf: None,
                            picture: None,
                            source_project_id: None,
                            description: None,
                            color: None,
                        });
                    }
                }
                // A project this device just created becomes the selection.
                if self.awaiting_new_project {
                    let known: HashSet<Id> = self.projects.iter().map(|p| p.id.clone()).collect();
                    if let Some(fresh) = projects.iter().find(|p| !known.contains(&p.id)) {
                        self.awaiting_new_project = false;
                        let id = fresh.id.clone();
                        self.projects = projects.clone();
                        self.select_project(id, cx);
                    }
                }
                self.projects = projects;
                self.arm_silence_end_timers(cx);
                // Rig (screenshots without a pointer): `ARBOS_OPEN_SPACE=<space id>` selects that
                // agent or people chat once it is known; `ARBOS_START_CALL=voice|video` then
                // starts a call there, as the header's call button would.
                if !self.rig_space_opened {
                    if let Ok(space) = std::env::var("ARBOS_OPEN_SPACE") {
                        if self.projects.iter().any(|p| p.id == space) {
                            self.rig_space_opened = true;
                            self.select_project(space, cx);
                            if let Ok(kind) = std::env::var("ARBOS_START_CALL") {
                                let kind = if kind == "video" {
                                    CallKind::Video
                                } else {
                                    CallKind::Voice
                                };
                                self.start_call_in_chat(kind, cx);
                            }
                        }
                    }
                }
                // Nothing in the centre yet: pick an agent. A channel open in the centre
                // (D88) has no selected project on purpose and stays.
                if self.selected_project.is_none() && self.selected_channel.is_none() {
                    let pick = self
                        .projects
                        .iter()
                        .find(|p| p.name == "Arbos")
                        .or(self.projects.last())
                        .map(|p| p.id.clone());
                    if let Some(id) = pick {
                        self.select_project(id, cx);
                    }
                }
                self.onboarding_registry_seen();
                self.onboarding_tick(cx);
                cx.notify();
            }
            SyncEvent::Machine(machine) => {
                self.machine = machine;
                self.onboarding_tick(cx);
                cx.notify();
            }
            SyncEvent::ChatSynced(chat_id) => {
                self.onboarding_chat_synced(&chat_id);
                self.onboarding_tick(cx);
            }
            SyncEvent::Workspace {
                project_id,
                running,
            } => {
                self.onboarding_workspace(&project_id, running);
                self.onboarding_tick(cx);
            }
            SyncEvent::GreetingWritten(chat_id) => {
                self.onboarding_greeting_written(&chat_id);
                self.onboarding_tick(cx);
            }
            SyncEvent::RestoreSelection {
                project_id,
                chat_id,
            } => {
                // Cold launch: open the agent's primary chat (same conversation
                // the iPhone shows — D78) so history matches across devices.
                // Multi-chat tabs stay available; last_chats only resumes a tab
                // within a session after the user picks one. A channel already
                // opened (D88) is not replaced.
                if self.selected_channel.is_none()
                    && self.projects.iter().any(|p| p.id == project_id)
                {
                    // Rig: `ARBOS_FIXTURE_OPEN=<project>/<chat id>` names the exact tab
                    // (a group under the agent, for example); the primary chat otherwise.
                    let explicit = std::env::var("ARBOS_FIXTURE_OPEN")
                        .ok()
                        .and_then(|spec| spec.split_once('/').map(|(_, c)| c.to_owned()))
                        .filter(|c| *c == chat_id);
                    let open = explicit
                        .filter(|id| self.tabs(&project_id).iter().any(|c| &c.id == id))
                        .or_else(|| {
                            self.primary_chat(&project_id)
                                .map(|c| c.id.clone())
                                .filter(|id| self.tabs(&project_id).iter().any(|c| &c.id == id))
                        })
                        .or_else(|| {
                            self.tabs(&project_id)
                                .iter()
                                .any(|c| c.id == chat_id)
                                .then_some(chat_id)
                        });
                    if let Some(id) = open {
                        self.active_tab.insert(project_id.clone(), id);
                    }
                    // select_project() keeps the tab within the visible chats.
                    self.select_project(project_id.clone(), cx);
                    // Rig: `ARBOS_OPEN_NEW_GROUP=1` opens the New group picker with one
                    // agent and one person already picked (member chips) — here,
                    // after the fixture's projects exist.
                    if std::env::var("ARBOS_OPEN_NEW_GROUP").is_ok_and(|v| v == "1") {
                        self.open_new_chat(cx);
                        self.open_new_group(cx);
                        let agent = self.my_projects().next().map(|p| p.id.clone());
                        let person = self.projects.iter().find_map(|p| p.person().cloned());
                        if let Some(id) = agent {
                            self.toggle_group_agent(id, cx);
                        }
                        if let Some(person) = person {
                            self.toggle_group_person(person, cx);
                        }
                    }
                    // Rig: `ARBOS_RIG_ADD_AGENT=<project id>[:<delay ms>]` adds that agent to
                    // the restored people chat the way the picker does — optimistic notice,
                    // relay call, reconciliation — after the delay (default 8 s, so a frame
                    // grabber is running), for the agent-added-notice recordings.
                    if let Ok(spec) = std::env::var("ARBOS_RIG_ADD_AGENT") {
                        if let Some(chat) = self.active_chat_id() {
                            let (pid, delay_ms) = match spec.split_once(':') {
                                Some((p, d)) => (p.to_owned(), d.parse().unwrap_or(8_000)),
                                None => (spec.clone(), 8_000u64),
                            };
                            cx.spawn(async move |this, cx| {
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(delay_ms))
                                    .await;
                                let _ = this.update(cx, |s, cx| s.add_agent(chat, pid, cx));
                            })
                            .detach();
                        }
                    }
                    // Rig: `ARBOS_REPLY_TO=1` arms a reply to the restored chat's
                    // last message (the composer's reply preview bar).
                    if std::env::var("ARBOS_REPLY_TO").is_ok_and(|v| v == "1") {
                        if let Some(chat) = self.active_chat_id() {
                            if let Some(last) = self.messages_of(&chat).last().map(|m| m.id.clone())
                            {
                                self.set_reply_to(chat, last, cx);
                            }
                        }
                    }
                    // Rig: `ARBOS_MEDIA_VIEWER=<n>` opens the restored chat's album at
                    // item n (chat order, 0-based), `=file` its first file;
                    // `ARBOS_MEDIA_VIEWER_FORWARD=1` opens the Forward picker too
                    // (feedback media-viewer-2026-10-23).
                    if let Ok(spec) = std::env::var("ARBOS_MEDIA_VIEWER") {
                        if let Some(chat) = self.active_chat_id() {
                            if spec == "file" {
                                let first_file = self.messages_of(&chat).iter().find_map(|m| {
                                    m.parts.iter().find_map(|p| match p {
                                        Part::Attachment { id, attachment }
                                            if !matches!(
                                                attachment.kind(),
                                                AttachmentKind::Image
                                                    | AttachmentKind::Video
                                                    | AttachmentKind::Audio
                                            ) =>
                                        {
                                            Some((m.id.clone(), id.clone()))
                                        }
                                        _ => None,
                                    })
                                });
                                if let Some((message_id, part_id)) = first_file {
                                    self.open_media_in_chat(&chat, &message_id, &part_id, cx);
                                }
                            } else if let Ok(ix) = spec.parse::<usize>() {
                                let album = self.media_album(&chat);
                                self.open_media_viewer(album, ix, cx);
                            }
                            if std::env::var("ARBOS_MEDIA_VIEWER_FORWARD").is_ok_and(|v| v == "1") {
                                self.media_viewer_toggle_forward(cx);
                            }
                        }
                    }
                    // Rig: `ARBOS_OPEN_PROFILE=agent[:model|reasoning|voice]|group[:library|:add-members|:member]|user[:block[:unblock]|:message|:view]|me[:view]`
                    // opens the right pane for the restored chat (selecting it
                    // closed whatever the shell opened at launch), and
                    // `ARBOS_OPEN_SETTINGS=…` re-opens my pane on its Settings tab.
                    if let Ok(spec) = std::env::var("ARBOS_OPEN_PROFILE") {
                        if let Some(rest) = spec.strip_prefix("agent:foreign") {
                            // `agent:foreign[:owner]` (rig, [feedback
                            // agent-pane-run-by-link-mac-2026-10-25]): the pane of
                            // the first agent in the restored group that someone
                            // else runs; `:owner` then follows its "Run by" link.
                            let me = self.me_handle();
                            let foreign = self
                                .projects
                                .iter()
                                .find(|p| p.id == project_id)
                                .and_then(|p| {
                                    p.agents.iter().find(|a| a.owner_handle != me).cloned()
                                });
                            if let Some(a) = foreign {
                                self.open_profile(
                                    ProfilePage::Agent {
                                        project_id: a.project_id.clone(),
                                    },
                                    cx,
                                );
                                if rest == ":owner" {
                                    self.push_profile(
                                        ProfilePage::User {
                                            handle: a.owner_handle.clone(),
                                            space_id: Some(a.project_id),
                                        },
                                        cx,
                                    );
                                }
                            }
                        } else if let Some(rest) = spec.strip_prefix("agent") {
                            self.open_profile(
                                ProfilePage::Agent {
                                    project_id: project_id.clone(),
                                },
                                cx,
                            );
                            self.agent_pick = rest.strip_prefix(':').and_then(AgentPick::parse);
                            if self.agent_pick == Some(AgentPick::Voice) {
                                self.fetch_voices();
                            }
                            // `agent:library` / `agent:computer` land on that tab.
                            match rest {
                                ":library" => self
                                    .set_profile_tab(crate::ui::profiles::agent::TAB_LIBRARY, cx),
                                ":computer" => self
                                    .set_profile_tab(crate::ui::profiles::agent::TAB_COMPUTER, cx),
                                _ => {}
                            }
                            // `agent:avatar[:generate|:upload[=<path>]]` opens the avatar picker
                            // sheet (D92 amendment) on that tab.
                            if let Some(tab) = rest.strip_prefix(":avatar") {
                                use crate::ui::avatar_picker::{Tab, Target};
                                let tab = match tab {
                                    ":generate" => Tab::Generate,
                                    ":upload" => Tab::Upload,
                                    t if t.starts_with(":upload=") => {
                                        self.rig_upload(&t[":upload=".len()..]);
                                        Tab::Upload
                                    }
                                    _ => Tab::Bot,
                                };
                                self.open_avatar_picker(Target::Agent(project_id), tab, cx);
                            }
                        } else if let Some(rest) = spec.strip_prefix("group") {
                            self.open_profile(
                                ProfilePage::Group {
                                    space_id: project_id.clone(),
                                },
                                cx,
                            );
                            if rest == ":library" {
                                self.set_profile_tab(1, cx);
                            } else if let Some(tab) = rest.strip_prefix(":avatar") {
                                // `group:avatar[:generate|:upload]` — the avatar picker (D92
                                // amendment) for the group's picture.
                                use crate::ui::avatar_picker::{Tab, Target};
                                let tab = match tab {
                                    ":generate" => Tab::Generate,
                                    ":upload" => Tab::Upload,
                                    _ => Tab::Bot,
                                };
                                self.open_avatar_picker(Target::Group(project_id), tab, cx);
                            } else if rest == ":add-members" {
                                // The pane's Add members sheet over the Members tab.
                                self.open_add_members(project_id, cx);
                            } else if let Some(menu) = rest.strip_prefix(":member-menu") {
                                self.rig_member_menu = Some(menu == ":confirm");
                            } else if rest == ":member" {
                                // `:member` lands on the first other member, pushed
                                // over the group as a click on its row would.
                                let me = self.me_handle();
                                if let Some(handle) = self
                                    .projects
                                    .iter()
                                    .find(|p| p.id == project_id)
                                    .and_then(|p| p.people.iter().find(|x| x.handle != me))
                                    .map(|x| x.handle.clone())
                                {
                                    self.push_profile(
                                        ProfilePage::User {
                                            handle,
                                            space_id: Some(project_id),
                                        },
                                        cx,
                                    );
                                }
                            }
                        } else if let Some(rest) = spec.strip_prefix("user") {
                            // `user[:block[:unblock]]` (#1545 rig): the DM peer's pane, then the
                            // same block / unblock commands the footer row sends.
                            if let Some(person) = self
                                .projects
                                .iter()
                                .find(|p| p.id == project_id)
                                .and_then(|p| p.person())
                                .cloned()
                            {
                                self.open_profile(
                                    ProfilePage::User {
                                        handle: person.handle.clone(),
                                        space_id: Some(project_id),
                                    },
                                    cx,
                                );
                                if rest.starts_with(":block") {
                                    self.block_user(person.handle.clone(), cx);
                                }
                                if rest == ":block:unblock" {
                                    self.rig_unblock = Some(person.clone());
                                }
                                // `user:message`: the hero's Message pill, pressed.
                                if rest == ":message" {
                                    self.start_dm(person.handle.clone(), cx);
                                }
                                // `user:view` — their picture full-size in the media viewer.
                                if rest == ":view" {
                                    let name = person.display_name();
                                    self.open_picture_viewer(&name, &person.avatar_url, cx);
                                }
                            }
                        } else if let Some(rest) = spec.strip_prefix("me") {
                            self.open_profile(ProfilePage::Me, cx);
                            // `me:view` — my picture full-size in the media viewer.
                            if rest == ":view" {
                                if let Some(a) = self.account.clone() {
                                    self.open_picture_viewer(&a.name, &a.avatar_url, cx);
                                }
                            }
                            // `me:avatar[:generate[=<prompt>]|:upload[=<path>]]` — the avatar
                            // picker for my own picture (Generate | Upload, Reset); with
                            // `=<prompt>` the shell types the prompt and presses Generate; with
                            // `=<path>` the Upload tab previews that file.
                            if let Some(tab) = rest.strip_prefix(":avatar") {
                                use crate::ui::avatar_picker::{Tab, Target};
                                let tab = match tab {
                                    ":upload" => Tab::Upload,
                                    t if t.starts_with(":upload=") => {
                                        self.rig_upload(&t[":upload=".len()..]);
                                        Tab::Upload
                                    }
                                    t => {
                                        if let Some(prompt) = t.strip_prefix(":generate=") {
                                            if !prompt.trim().is_empty() {
                                                self.avatar_rig_prompt = Some(prompt.to_owned());
                                            }
                                        }
                                        Tab::Generate
                                    }
                                };
                                let me = self
                                    .account
                                    .as_ref()
                                    .map(|a| a.id.clone())
                                    .unwrap_or_default();
                                self.open_avatar_picker(Target::Me(me), tab, cx);
                            }
                        }
                    } else if std::env::var_os("ARBOS_OPEN_SETTINGS").is_some() {
                        self.open_profile(ProfilePage::Me, cx);
                        self.set_profile_tab(1, cx);
                    }
                }
            }
            SyncEvent::ChatPrefs {
                last_chats,
                archived,
            } => {
                self.last_chats = last_chats;
                self.archived_chats = archived.into_iter().collect();
                let projects: Vec<Id> = self.projects.iter().map(|p| p.id.clone()).collect();
                for p in projects {
                    self.enforce_tab(&p);
                }
                cx.notify();
            }
            SyncEvent::Pins { pins, cap } => {
                self.pins = pins;
                self.pin_cap = cap.max(1);
                cx.notify();
            }
            SyncEvent::StoreUsage { bytes, chats } => {
                self.store_usage = (bytes, chats);
                cx.notify();
            }
            SyncEvent::ReleasePublished { platform, version } => {
                // The relay says a build is out: check the appcast now instead of at the next poll.
                if platform == "mac" {
                    update::log(format!(
                        "release_published push: {version}; checking the feed"
                    ));
                    self.check_for_updates(cx);
                }
            }
            SyncEvent::ProjectStatus {
                project_id,
                state,
                detail,
                progress,
            } => {
                let failed = state == BoxState::Error
                    && self
                        .project(&project_id)
                        .is_some_and(|p| p.state != BoxState::Error);
                if let Some(p) = self.projects.iter_mut().find(|p| p.id == project_id) {
                    p.state = state;
                    p.state_detail = detail.clone();
                    p.wake_progress = progress;
                }
                if failed {
                    self.notify(
                        project_id,
                        None,
                        NotifyKind::WakeFailed,
                        detail.unwrap_or_else(|| "The box did not come up".into()),
                    );
                }
                cx.notify();
            }
            SyncEvent::Chats { project_id, chats } => {
                let active = self.active_tab.get(&project_id).cloned();
                let still_there = active
                    .as_ref()
                    .map(|a| chats.iter().any(|c| &c.id == a))
                    .unwrap_or(false);
                if !still_there {
                    if let Some(first) = chats.first() {
                        self.active_tab.insert(project_id.clone(), first.id.clone());
                    }
                }
                // Keep the order the strip already shows (a new chat sits last,
                // beside "+"); chats the relay no longer lists leave, new ones append.
                let chats = match self.chats.get(&project_id) {
                    Some(current) if !current.is_empty() => {
                        let mut merged: Vec<Chat> = current
                            .iter()
                            .filter_map(|c| chats.iter().find(|n| n.id == c.id).cloned())
                            .collect();
                        let known: HashSet<Id> = merged.iter().map(|m| m.id.clone()).collect();
                        merged.extend(chats.iter().filter(|n| !known.contains(&n.id)).cloned());
                        merged
                    }
                    _ => chats,
                };
                // §9c: the relay's `archived_at` is the truth once it has one;
                // a chat it now lists as not archived leaves the local set only
                // when it was archived on the relay before (older relays never set it).
                for c in &chats {
                    match c.archived_at {
                        Some(_) => {
                            self.archived_chats.insert(c.id.clone());
                        }
                        None => {
                            let was_remote = self
                                .chats
                                .get(&project_id)
                                .and_then(|cur| cur.iter().find(|x| x.id == c.id))
                                .is_some_and(|x| x.archived_at.is_some());
                            if was_remote {
                                self.archived_chats.remove(&c.id);
                            }
                        }
                    }
                }
                self.chats.insert(project_id.clone(), chats);
                self.enforce_tab(&project_id);
                self.sync_badge();
                self.onboarding_tick(cx);
                cx.notify();
            }
            SyncEvent::Voices { default, voices } => {
                self.voices = voices;
                self.default_voice = default;
                cx.notify();
            }
            SyncEvent::VoiceSample { url, path } => {
                self.sample_paths.insert(url.clone(), path.clone());
                if self.sample_wanted.as_deref() == Some(url.as_str()) {
                    self.sample_wanted = None;
                    self.sample_player = Some(SamplePlayer::play(path));
                    self.sample_playing = Some(url);
                }
                cx.notify();
            }
            SyncEvent::SpaceArchived {
                space_id,
                archived_at,
            } => {
                // Another device archived / unarchived a sidebar space for me.
                if let Some(p) = self.projects.iter_mut().find(|p| p.id == space_id) {
                    p.archived_at = archived_at;
                } else if let Some(s) = self.servers.iter_mut().find(|s| s.id == space_id) {
                    s.archived_at = archived_at;
                }
                cx.notify();
            }
            SyncEvent::ChatArchived {
                chat_id,
                archived_at,
            } => {
                // Another device archived / unarchived it: the tab goes or returns.
                if archived_at.is_some() {
                    self.archived_chats.insert(chat_id.clone());
                } else {
                    self.archived_chats.remove(&chat_id);
                }
                if let Some(c) = self
                    .chats
                    .values_mut()
                    .flat_map(|v| v.iter_mut())
                    .find(|c| c.id == chat_id)
                {
                    c.archived_at = archived_at;
                }
                // A people chat is its own sidebar row: the row hides or returns with it.
                if let Some(p) = self
                    .projects
                    .iter_mut()
                    .find(|p| p.id == chat_id && p.is_people())
                {
                    p.archived_at = archived_at;
                }
                if let Some(project_id) = self.project_of_chat(&chat_id).map(|p| p.id.clone()) {
                    self.enforce_tab(&project_id);
                }
                cx.notify();
            }
            SyncEvent::NotificationsCleared { chat_id, badge } => {
                // I read it on another device: its banners go, the dot clears, the Dock
                // badge follows. The relay's total is the truth for what this Mac cannot
                // see yet (`/me/unread` confirms on the next refresh).
                notify::withdraw_chat(&chat_id);
                if let Some(c) = self
                    .chats
                    .values_mut()
                    .flat_map(|v| v.iter_mut())
                    .find(|c| c.id == chat_id)
                {
                    c.unread = false;
                    c.unread_count = 0;
                }
                let local: usize = self
                    .chats
                    .values()
                    .flat_map(|v| v.iter())
                    .map(Chat::missed_count)
                    .sum();
                notify::set_badge(local.min(badge as usize));
                cx.notify();
            }
            SyncEvent::ChatSnapshot {
                chat_id,
                mut messages,
            } => {
                if trace_rows() {
                    eprintln!(
                        "rows: app-snapshot chat={chat_id} n={} at={}",
                        messages.len(),
                        now_ms()
                    );
                }
                self.fold_optimistic_notices(&chat_id, &mut messages);
                let dropped = !messages.is_empty();
                let ui = self.chat_ui.entry(chat_id.clone()).or_default();
                if dropped {
                    ui.composer_dropped = true;
                }
                // Whole messages inserted at once (a person's line, a shared
                // agent's reply) arrive as a new snapshot, not an upsert: tell
                // the notifier about the ones we have not seen. A chat's first
                // snapshot is history, except what was written just now.
                //
                // Append-only catch-up snapshots (#1405 /
                // feedback app-01M5Z3QPDA5BRE84SAPADXX05V): when the previous
                // list is a prefix of the new one, skip building a HashSet of
                // every id — the new suffix is exactly what arrived.
                let first_load = !self.messages.contains_key(&chat_id);
                let fresh_after = now_ms().saturating_sub(SNAPSHOT_FRESH_MS);
                let arrived: Vec<Message> = {
                    let prev = self.messages.get(&chat_id);
                    let append_only = prev.is_some_and(|old| {
                        messages.len() >= old.len()
                            && old.iter().zip(messages.iter()).all(|(a, b)| a.id == b.id)
                    });
                    if append_only {
                        let old_len = prev.map(|p| p.len()).unwrap_or(0);
                        messages[old_len..]
                            .iter()
                            .filter(|m| !first_load || m.created_at >= fresh_after)
                            .filter(|m| !m.is_streaming())
                            .cloned()
                            .collect()
                    } else {
                        let seen: HashSet<Id> = prev
                            .map(|l| l.iter().map(|m| m.id.clone()).collect())
                            .unwrap_or_default();
                        messages
                            .iter()
                            .filter(|m| !seen.contains(&m.id))
                            .filter(|m| !first_load || m.created_at >= fresh_after)
                            .filter(|m| !m.is_streaming())
                            .cloned()
                            .collect()
                    }
                };
                self.messages.insert(chat_id.clone(), messages);
                self.index_chat(&chat_id);
                // Message counts decide an agent's primary chat (single-chat mode).
                if let Some(project_id) = self.project_of_chat(&chat_id).map(|p| p.id.clone()) {
                    self.enforce_tab(&project_id);
                }
                for m in &arrived {
                    if self.hides_message(&chat_id, m) {
                        continue;
                    }
                    self.notify_for_message(&chat_id, m, false, false, true);
                }
                let chat_visible = self.chat_visible_in_active_window(&chat_id);
                let effects: Vec<ArrivalRead> = arrived
                    .iter()
                    .map(|m| {
                        arrival_read(
                            chat_visible,
                            self.is_mine(m),
                            m.role == Role::System || self.hides_message(&chat_id, m),
                        )
                    })
                    .collect();
                self.apply_arrival_read(&chat_id, &effects);
                if effects.contains(&ArrivalRead::MarkUnread) {
                    self.unarchive_on_activity(&chat_id, cx);
                }
                self.request_thumbnails(&chat_id);
                let all = self.messages.get(&chat_id).cloned().unwrap_or_default();
                self.request_link_icons_for(&all);
                if self.onboarding.is_some() {
                    self.onboarding_tick(cx);
                }
                cx.emit(AppEvent::TranscriptChanged { chat_id });
                cx.notify();
            }
            SyncEvent::MessageUpsert { chat_id, message } => {
                let mut message = *message;
                // Optimistic send stores a decoded picture under `local:…`. Keep it on
                // the row when the server frame arrives so the card does not flash
                // "Loading preview…" / "No preview" after the user just added the image.
                let prior = self
                    .messages
                    .get(&chat_id)
                    .and_then(|list| list.iter().find(|m| m.id == message.id))
                    .cloned();
                if let Some(ref prior) = prior {
                    self.carry_local_previews(prior, &mut message);
                }
                self.request_thumbnails_for(&chat_id, std::slice::from_ref(&message));
                self.request_link_icons_for(std::slice::from_ref(&message));
                let list = self.messages.entry(chat_id.clone()).or_default();
                let mut text_only = false;
                let mut appended = 0;
                let mut was_streaming = false;
                let mut had_open_question = false;
                let mut is_new = false;
                match list.iter_mut().find(|m| m.id == message.id) {
                    Some(existing) => {
                        let old_len = existing.text_len();
                        let new_len = message.text_len();
                        text_only = existing.status == message.status
                            && existing.parts.len() == message.parts.len()
                            && new_len > old_len
                            && same_shape(existing, &message);
                        appended = new_len.saturating_sub(old_len);
                        was_streaming = existing.is_streaming();
                        had_open_question = needs_input(existing);
                        *existing = message.clone();
                    }
                    None => {
                        is_new = true;
                        list.push(message.clone());
                    }
                }
                self.index_message(&chat_id, &message);
                let hidden = self.hides_message(&chat_id, &message);
                if !hidden {
                    self.notify_for_message(
                        &chat_id,
                        &message,
                        was_streaming,
                        had_open_question,
                        is_new,
                    );
                }
                if is_new {
                    let effect = arrival_read(
                        self.chat_visible_in_active_window(&chat_id),
                        self.is_mine(&message),
                        message.role == Role::System || hidden,
                    );
                    self.apply_arrival_read(&chat_id, &[effect]);
                    if effect == ArrivalRead::MarkUnread {
                        self.unarchive_on_activity(&chat_id, cx);
                    }
                }
                if text_only && appended > 0 {
                    cx.emit(AppEvent::TranscriptText { chat_id });
                } else {
                    cx.emit(AppEvent::TranscriptChanged { chat_id });
                    cx.notify();
                }
            }
            SyncEvent::Link(link) => {
                self.link = link;
                cx.notify();
            }
            SyncEvent::RoomLink { key, state } => {
                let now = now_ms();
                let changed = self.room_links.get(&key).map(|(s, _)| *s) != Some(state);
                if changed {
                    self.room_links.insert(key, (state, now));
                }
                // A room that is live proves the relay is reachable: a verdict
                // from an older HTTP poll must not keep "Relay down" up.
                if state == LinkState::Live && self.relay_link.0 != LinkState::Live {
                    self.relay_link = (LinkState::Live, now);
                }
                if changed {
                    self.link_changed(state, cx);
                }
            }
            SyncEvent::RelayLink(state) => {
                if self.relay_link.0 != state {
                    self.relay_link = (state, now_ms());
                    self.link_changed(state, cx);
                }
            }
            SyncEvent::Connectors(view) => {
                self.connectors = view;
                cx.notify();
            }
            SyncEvent::ConnectorAuth { provider, auth_url } => {
                cx.open_url(&auth_url);
                self.push_notice(format!(
                    "Opened the browser to connect {provider}; the relay finishes the sign-in."
                ));
                cx.notify();
            }
            SyncEvent::ProjectSecrets { project_id, view } => {
                self.project_secrets.insert(project_id, view);
            }
            SyncEvent::ShellGrants { project_id, items } => {
                self.shell_grants.insert(project_id, items);
                cx.notify();
            }
            SyncEvent::Secrets(view) => {
                self.secrets = view;
                cx.notify();
            }
            SyncEvent::Devices(result) => {
                match result {
                    Ok(items) => {
                        self.devices.items = items;
                        self.devices.error = None;
                    }
                    Err(why) => self.devices.error = Some(why),
                }
                self.devices.loaded = true;
                cx.notify();
            }
            SyncEvent::Routines {
                project_id,
                routines,
            } => {
                self.routines.insert(project_id, routines);
                cx.notify();
            }
            SyncEvent::BoxReport(report) => {
                self.box_report = Some(report);
                cx.notify();
            }
            SyncEvent::ProjectJobs { project_id, jobs } => {
                if self.jobs.get(&project_id).is_some_and(|j| *j == jobs) {
                    return;
                }
                if jobs.is_empty() {
                    self.jobs.remove(&project_id);
                } else {
                    self.jobs.insert(project_id, jobs);
                }
                // The drawer lists live work only: nothing open → it closes itself.
                if self
                    .tasks_open
                    .as_deref()
                    .is_some_and(|chat| self.open_jobs(chat).is_empty())
                {
                    self.tasks_open = None;
                }
                // Rig: `ARBOS_RIG_TASKS=open` opens the drawer on the first listing.
                if self.tasks_open.is_none() && crate::ui::tasks::rig_setting() == Some("open") {
                    if let Some(chat) = self
                        .active_chat_id()
                        .filter(|c| !self.open_jobs(c).is_empty())
                    {
                        self.tasks_open = Some(chat);
                    }
                }
                // Every listed job has its tab (`jobtabs`).
                for project_id in self.jobs.keys().cloned().collect::<Vec<_>>() {
                    self.sync_job_tabs(&project_id);
                }
                // Rig: `ARBOS_RIG_JOBTAB=open` selects the newest job's tab once.
                if self.rollouts.is_empty()
                    && std::env::var("ARBOS_RIG_JOBTAB").is_ok_and(|v| v == "open")
                {
                    if let Some(chat) = self
                        .active_chat_id()
                        .filter(|c| !self.open_jobs(c).is_empty())
                    {
                        self.open_newest_job_tab(chat, cx);
                    }
                }
                cx.notify();
            }
            SyncEvent::JobRollout {
                project_id,
                job_id,
                rollout,
            } => {
                // A document for a job no listing named yet still gets its tab.
                if self.job_project(&job_id).is_none() && !rollout.unavailable {
                    let tab = crate::jobtabs::tab(&project_id, &job_id, &rollout.title, now_ms());
                    self.job_tabs.entry(project_id).or_default().push(tab);
                }
                self.apply_rollout(&job_id, rollout, cx);
            }
            SyncEvent::RoutineRuns { routine_id, runs } => {
                self.routine_runs.insert(routine_id, runs);
                cx.notify();
            }
            SyncEvent::Skills(mut skills) => {
                skills.sort_by_key(|s| s.name.to_lowercase());
                self.skills = skills;
                cx.notify();
            }
            SyncEvent::Channels(view) => {
                self.channels = view;
                cx.notify();
            }
            SyncEvent::LocalDevices(view) => {
                self.local = view;
                cx.notify();
            }
            SyncEvent::Transcript {
                request_id,
                text,
                error,
                mock,
            } => {
                cx.emit(AppEvent::VoiceTranscript {
                    request_id,
                    text,
                    error,
                    mock,
                });
            }
            SyncEvent::Speech {
                request_id,
                bytes,
                mime,
                error,
                mock,
            } => {
                cx.emit(AppEvent::VoiceSpeech {
                    request_id,
                    bytes,
                    mime,
                    error,
                    mock,
                });
            }
            SyncEvent::ServersAvailable(on) => {
                self.servers_available = on;
                if !on {
                    let gone: Vec<Id> = self.servers.iter().map(|s| s.id.clone()).collect();
                    for id in gone {
                        self.drop_server(&id, cx);
                    }
                    self.server_sheet = None;
                }
                cx.notify();
            }
            SyncEvent::Servers(list) => self.on_servers(list, cx),
            SyncEvent::ServerDetail(server) => self.on_server_detail(server, cx),
            SyncEvent::ServerOpened(server) => self.on_server_opened(server, cx),
            SyncEvent::ServerError(err) => {
                self.server_busy = false;
                // Inline in the open sheet; a toast when no sheet is up to hold it.
                if self.server_sheet.is_none() {
                    self.push_failure(err.clone(), None);
                }
                self.server_error = Some(error_note::Slot::new(err));
                cx.notify();
            }
            SyncEvent::MatrixRooms(snapshot) => self.on_matrix_rooms(snapshot, cx),
            SyncEvent::MatrixTimeline { room_id, rows } => {
                self.on_matrix_timeline(room_id, rows, cx)
            }
            SyncEvent::ChannelHistoryEnd { room_id } => self.chat_search_history_end(&room_id, cx),
            SyncEvent::MatrixConnection { connected, error } => {
                if let (false, Some(err)) = (connected, &error) {
                    log::warn!("servers: homeserver connection: {err}");
                }
                if self.matrix_connected != connected {
                    self.matrix_connected = connected;
                    cx.notify();
                }
            }
            SyncEvent::ChannelSendFailed {
                room_id,
                local_id,
                error,
            } => {
                log::warn!("servers: send to {room_id} failed: {}", error.raw);
                // A channel's chat id is its room id (`active_chat_id`).
                self.send_errors
                    .insert(room_id.clone(), error_note::Slot::new(error));
                self.on_channel_send_failed(room_id, local_id, cx)
            }
            SyncEvent::MessageSendFailed {
                chat_id,
                message_id,
                error,
            } => {
                log::warn!("send to {chat_id} failed: {}", error.raw);
                if let Some(list) = self.messages.get_mut(&chat_id) {
                    if let Some(m) = list.iter_mut().find(|m| m.id == message_id) {
                        m.pending = false;
                        m.status = MessageStatus::Error;
                    }
                }
                self.send_errors
                    .insert(chat_id.clone(), error_note::Slot::new(error));
                cx.emit(AppEvent::TranscriptChanged { chat_id });
                cx.notify();
            }
        }
    }

    // ----- selectors -------------------------------------------------------

    pub fn project(&self, id: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id).or_else(|| {
            self.draft_dm
                .as_ref()
                .filter(|d| d.id() == id)
                .map(|d| &d.space)
        })
    }

    pub fn selected(&self) -> Option<&Project> {
        self.selected_project
            .as_deref()
            .and_then(|id| self.project(id))
    }

    pub fn tabs(&self, project_id: &str) -> &[Chat] {
        self.chats
            .get(project_id)
            .map(|v| v.as_slice())
            .or_else(|| {
                self.draft_dm
                    .as_ref()
                    .filter(|d| d.id() == project_id)
                    .map(|d| d.chat.as_slice())
            })
            .unwrap_or(&[])
    }

    /// The chats the UI may show for a space: all of them with multi-chat on,
    /// or for a people chat; for an agent otherwise (§9b) its primary chat, the
    /// group chats it hosts and the "Projects ⇄" system tab — only the extra
    /// plain chats are hidden.
    pub fn visible_tabs(&self, project_id: &str) -> Vec<&Chat> {
        let is_people = self.project(project_id).is_some_and(|p| p.is_people());
        if is_people {
            return self.tabs(project_id).iter().collect();
        }
        let primary = self.primary_chat(project_id);
        if self.multi_chat {
            // Archived chats leave the strip; the pinned "Main" chat never does.
            // Job tabs (`jobtabs`) follow the agent's chats.
            return self
                .tabs(project_id)
                .iter()
                .filter(|c| c.is_pinned || !self.archived_chats.contains(&c.id))
                .chain(self.job_tabs.get(project_id).into_iter().flatten())
                .collect();
        }
        let mut out: Vec<&Chat> = primary.into_iter().collect();
        out.extend(self.tabs(project_id).iter().filter(|c| {
            (c.is_group() || c.kind == ChatKind::Projects || c.is_system)
                && primary.is_none_or(|p| p.id != c.id)
        }));
        out
    }

    /// The one chat of an agent. The relay's `primary_chat_id` wins when it names
    /// a chat we have; otherwise the pinned chat, else the plain chat with the
    /// most messages, ties → earliest created (ULID order). Groups and system
    /// tabs only when nothing else exists.
    pub fn primary_chat(&self, project_id: &str) -> Option<&Chat> {
        let tabs = self.tabs(project_id);
        if let Some(named) = self
            .project(project_id)
            .and_then(|p| p.primary_chat.as_deref())
            .and_then(|id| tabs.iter().find(|c| c.id == id))
        {
            return Some(named);
        }
        let plain: Vec<&Chat> = tabs
            .iter()
            .filter(|c| !c.is_group() && !c.is_system)
            .collect();
        // No relay verdict: the pinned "Main" chat is the agent's chat (a new
        // project's pinned chat is its primary from creation, §9b).
        if let Some(pinned) = plain.iter().find(|c| c.is_pinned) {
            return Some(pinned);
        }
        let pool = if plain.is_empty() {
            tabs.iter().collect()
        } else {
            plain
        };
        pool.into_iter().min_by(|a, b| {
            let (na, nb) = (self.messages_of(&a.id).len(), self.messages_of(&b.id).len());
            nb.cmp(&na).then_with(|| a.id.cmp(&b.id))
        })
    }

    /// The agent's archived chats (the "+" dropdown's previous chats).
    /// Never-used empty "New chat" rows are omitted — they are discarded on
    /// close, and older piles must not keep showing up in Previous chats.
    pub fn archived_tabs(&self, project_id: &str) -> Vec<&Chat> {
        self.tabs(project_id)
            .iter()
            .filter(|c| {
                !c.is_pinned && self.archived_chats.contains(&c.id) && !self.is_unused_new_chat(c)
            })
            .collect()
    }

    /// A plain chat that still has the default title and no local messages —
    /// opened with "+" / ⌘T and never used. Title alone is enough when the
    /// transcript has not been loaded yet (archived empties); a loaded
    /// non-empty transcript means the title failed to update and is kept.
    fn is_unused_new_chat(&self, chat: &Chat) -> bool {
        if chat.is_pinned || chat.is_system || chat.is_group() || chat.kind != ChatKind::Chat {
            return false;
        }
        if chat.title != "New chat" {
            return false;
        }
        match self.messages.get(&chat.id) {
            None => true,
            Some(msgs) => msgs.is_empty(),
        }
    }

    /// An unused "New chat" to reopen instead of minting another: prefer a
    /// visible empty tab, else an archived one.
    fn reusable_new_chat(&self, project_id: &str) -> Option<Id> {
        let tabs = self.tabs(project_id);
        let visible = tabs
            .iter()
            .find(|c| !self.archived_chats.contains(&c.id) && self.is_unused_new_chat(c));
        if let Some(c) = visible {
            return Some(c.id.clone());
        }
        tabs.iter()
            .find(|c| self.archived_chats.contains(&c.id) && self.is_unused_new_chat(c))
            .map(|c| c.id.clone())
    }

    /// Archive a chat on this Mac (the tab's "×", ⌘W): it leaves the strip and
    /// waits in the "+" dropdown; nothing is deleted or closed on the relay.
    /// Never-used empty chats are closed instead — they must not pile up in
    /// Previous chats.
    pub fn archive_chat(&mut self, chat_id: Id, cx: &mut Context<Self>) {
        let Some(project_id) = self.project_of_chat(&chat_id).map(|p| p.id.clone()) else {
            return;
        };
        if self
            .tabs(&project_id)
            .iter()
            .any(|c| c.id == chat_id && c.is_pinned)
        {
            return;
        }
        if self
            .tabs(&project_id)
            .iter()
            .any(|c| c.id == chat_id && self.is_unused_new_chat(c))
        {
            self.delete_chat_tab(chat_id, cx);
            return;
        }
        self.archived_chats.insert(chat_id.clone());
        if let Some(c) = self
            .chats
            .get_mut(&project_id)
            .and_then(|v| v.iter_mut().find(|c| c.id == chat_id))
        {
            c.archived_at = Some(now_ms());
        }
        self.command(ClientCommand::ArchiveChat {
            chat_id,
            archived: true,
        });
        self.enforce_tab(&project_id);
        cx.notify();
    }

    /// Back onto the strip, and open it.
    pub fn unarchive_chat(&mut self, chat_id: Id, cx: &mut Context<Self>) {
        if !self.archived_chats.remove(&chat_id) {
            return;
        }
        if let Some(c) = self
            .chats
            .values_mut()
            .flat_map(|v| v.iter_mut())
            .find(|c| c.id == chat_id)
        {
            c.archived_at = None;
        }
        self.command(ClientCommand::ArchiveChat {
            chat_id: chat_id.clone(),
            archived: false,
        });
        if let Some(project_id) = self.project_of_chat(&chat_id).map(|p| p.id.clone()) {
            if self.selected_project.as_deref() == Some(project_id.as_str()) {
                self.select_tab(chat_id, cx);
                return;
            }
        }
        cx.notify();
    }

    /// The active tab of an agent is always one of its visible chats. When it
    /// is not (nothing picked yet, archived, hidden by single-chat mode) fall
    /// back to: the primary chat (same one the iPhone opens — D78), then the
    /// chat last used on this Mac, then the first visible one. Primary first
    /// keeps history in sync across devices; last_chats only matters when the
    /// relay has no primary yet. Returns true when the active tab changed.
    fn enforce_tab(&mut self, project_id: &str) -> bool {
        if self.project(project_id).is_none_or(|p| p.is_people()) {
            return false;
        }
        let visible: Vec<Id> = self
            .visible_tabs(project_id)
            .iter()
            .map(|c| c.id.clone())
            .collect();
        if visible.is_empty() {
            return false;
        }
        // A search hit may open a chat the strip would hide until another tab
        // is chosen.
        if self
            .active_tab
            .get(project_id)
            .is_some_and(|a| visible.contains(a) || self.search_chat.as_deref() == Some(a.as_str()))
        {
            return false;
        }
        let pick = self
            .primary_chat(project_id)
            .map(|c| c.id.clone())
            .filter(|c| visible.contains(c))
            .or_else(|| {
                self.last_chats
                    .get(project_id)
                    .filter(|c| visible.contains(c))
                    .cloned()
            })
            .unwrap_or_else(|| visible[0].clone());
        self.active_tab.insert(project_id.to_string(), pick.clone());
        if self.selected_project.as_deref() == Some(project_id) {
            self.command(ClientCommand::OpenChat { chat_id: pick });
        }
        true
    }

    pub fn active_chat(&self) -> Option<&Chat> {
        let project_id = self.selected_project.as_deref()?;
        let chat_id = self.active_tab.get(project_id)?;
        self.tabs(project_id)
            .iter()
            .chain(self.job_tabs.get(project_id).into_iter().flatten())
            .find(|c| &c.id == chat_id)
    }

    /// The chat the centre shows: a channel's room id when one is open (D88), else the
    /// selected project's active tab.
    pub fn active_chat_id(&self) -> Option<Id> {
        if let Some(sel) = &self.selected_channel {
            return sel.room_id.clone();
        }
        self.active_chat().map(|c| c.id.clone())
    }

    /// The rail space a chat lives in: its project for project chats, itself for people chats.
    pub fn space_of_chat(&self, chat_id: &str) -> Id {
        self.chats
            .iter()
            .chain(self.job_tabs.iter())
            .find(|(_, tabs)| tabs.iter().any(|c| c.id == chat_id))
            .map(|(space, _)| space.clone())
            .unwrap_or_else(|| chat_id.to_owned())
    }

    pub fn messages_of(&self, chat_id: &str) -> &[Message] {
        self.messages
            .get(chat_id)
            .or_else(|| self.channel_rows.get(chat_id))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// What the header's status pill says for `chat_id`, or `None` when the
    /// link is fine. The room's own socket and the relay's HTTP poll each
    /// vote; the highest-priority verdict wins (offline > relay down >
    /// reconnecting > syncing). A state younger than [`LINK_PILL_GRACE_MS`]
    /// does not show, so a 250 ms redial or a sub-second catch-up never
    /// flashes a pill.
    pub fn link_pill(&self, chat_id: Option<&str>) -> Option<LinkState> {
        let now = now_ms();
        let settled = |(state, since): (LinkState, u64)| {
            (state != LinkState::Live && now.saturating_sub(since) >= LINK_PILL_GRACE_MS)
                .then_some(state)
        };
        let room = chat_id
            .and_then(|id| self.room_links.get(id).copied())
            .and_then(settled);
        let relay = settled(self.relay_link);
        room.max(relay)
    }

    /// A link state changed: repaint, and repaint again once the grace period
    /// is over so the pill appears (or stays away) without a user event.
    fn link_changed(&mut self, state: LinkState, cx: &mut Context<Self>) {
        cx.notify();
        if state == LinkState::Live {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(LINK_PILL_GRACE_MS + 50))
                .await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    pub fn chat_is_streaming(&self, chat_id: &str) -> bool {
        Message::chat_is_actively_working(self.messages_of(chat_id), now_ms())
    }

    pub fn project_is_streaming(&self, project_id: &str) -> bool {
        self.tabs(project_id)
            .iter()
            .any(|c| self.chat_is_streaming(&c.id))
    }

    pub fn project_unread(&self, project_id: &str) -> usize {
        self.visible_tabs(project_id)
            .iter()
            .filter(|c| c.unread)
            .count()
    }

    /// S8: the last assistant message ended with an `ask_user` question and
    /// nothing has been sent since. A typed widget question is answered from
    /// its card, so it does not switch the composer.
    pub fn chat_needs_answer(&self, chat_id: &str) -> bool {
        match self.messages_of(chat_id).last() {
            Some(m) if m.role == Role::Assistant && !m.is_streaming() => {
                if m.has_question_part() {
                    m.open_questions()
                        .any(|p| matches!(p, Part::Question { widget: None, .. }))
                } else {
                    m.parts.iter().any(|p| {
                        matches!(
                            p,
                            Part::Status {
                                code: StatusKind::AskUser,
                                ..
                            }
                        )
                    })
                }
            }
            Some(_) | None => false,
        }
    }

    /// The newest open plain question's id in the last assistant message (S8 composer answer).
    fn open_plain_question_id(&self, chat_id: &str) -> Option<String> {
        let m = self.messages_of(chat_id).last()?;
        m.open_questions().rev().find_map(|p| match p {
            Part::Question {
                question_id,
                widget: None,
                ..
            } => Some(question_id.clone()),
            _ => None,
        })
    }

    /// S18: the newest assistant message carries an approval card nobody has decided yet.
    pub fn chat_needs_approval(&self, chat_id: &str) -> bool {
        match self.messages_of(chat_id).last() {
            Some(m) if m.role == Role::Assistant && !m.is_streaming() => {
                m.pending_approvals().next().is_some()
            }
            Some(_) | None => false,
        }
    }

    pub fn composer_mode(&self, chat_id: &str) -> ComposerMode {
        if self.chat_needs_answer(chat_id) {
            ComposerMode::Answer
        } else if self.chat_needs_approval(chat_id) {
            ComposerMode::Approve
        } else {
            ComposerMode::Send
        }
    }

    pub fn chat_ui(&self, chat_id: &str) -> ChatUi {
        self.chat_ui.get(chat_id).cloned().unwrap_or_default()
    }

    pub fn window_is_narrow(&self, width: f32) -> bool {
        width < crate::ui::theme::NARROW_WINDOW
    }

    // ----- commands --------------------------------------------------------

    pub(crate) fn command(&self, cmd: ClientCommand) {
        if crate::draft_dm::names_draft(&cmd) {
            return;
        }
        let _ = self.commands.send(cmd);
    }

    pub fn select_project(&mut self, project_id: Id, cx: &mut Context<Self>) {
        if !crate::draft_dm::is_draft(&project_id) {
            self.drop_draft_dm();
        }
        let switching = self.selected_project.as_deref() != Some(project_id.as_str())
            || self.selected_channel.is_some();
        // A chat replaces the channel in the centre (D88).
        self.selected_channel = None;
        if switching {
            self.tasks_open = None;
            self.search_chat = None;
            // The right pane belongs to the chat it was opened from; a new
            // chat starts without one.
            self.close_profile(cx);
        }
        self.reveal_message = None;
        self.highlight_message = None;
        // The Search-in-chat bar belongs to the chat it was opened in.
        self.chat_search = None;
        if self
            .renaming_project
            .as_ref()
            .is_some_and(|id| id != &project_id)
        {
            self.renaming_project = None;
        }
        self.selected_project = Some(project_id.clone());
        self.sidebar_overlay = false;
        // Single-chat agents: open the primary chat so Mac matches iPhone (D78).
        // Multi-chat: keep the tab last used here (active_tab / last_chats) so
        // leaving for another agent and coming back restores that sub-chat.
        // Re-clicking the already-selected agent keeps the tab either way.
        if switching
            && !self.multi_chat
            && self.project(&project_id).is_some_and(|p| !p.is_people())
        {
            if let Some(primary) = self.primary_chat(&project_id).map(|c| c.id.clone()) {
                self.active_tab.insert(project_id.clone(), primary.clone());
                self.last_chats.insert(project_id.clone(), primary);
            }
        }
        // Agents: keep a valid tab; else primary, else last used, else first.
        if self.enforce_tab(&project_id) {
            self.sync_focus();
            cx.notify();
            return;
        }
        if let Some(chat_id) = self.active_tab.get(&project_id).cloned() {
            self.command(ClientCommand::OpenChat { chat_id });
        } else if let Some(first) = self.tabs(&project_id).first() {
            let id = first.id.clone();
            self.active_tab.insert(project_id, id.clone());
            self.command(ClientCommand::OpenChat { chat_id: id });
        }
        self.sync_focus();
        cx.notify();
    }

    /// Home-search row click: open the chat that holds the matching message and
    /// ask the transcript to scroll to it. Name-only matches open normally.
    pub fn open_from_home_search(&mut self, project_id: Id, query: &str, cx: &mut Context<Self>) {
        let q = search::normalize_query(query);
        let hit = if q.is_empty() {
            None
        } else {
            self.find_message_match(&project_id, &q)
        };
        let Some((chat_id, message_id)) = hit else {
            self.select_project(project_id, cx);
            return;
        };
        self.reveal_search_message(project_id, chat_id, message_id, cx);
    }

    /// Click on a reply bubble's quote header: scroll to the original in the
    /// open chat and highlight it for a moment.
    pub fn reveal_reply(&mut self, chat_id: Id, message_id: Id, cx: &mut Context<Self>) {
        self.highlight_message = Some(message_id.clone());
        self.reveal_message = Some((chat_id, message_id.clone()));
        self.reveal_nonce += 1;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(QUOTE_HIGHLIGHT_MS))
                .await;
            let _ = this.update(cx, |state, cx| {
                if state.highlight_message.as_deref() == Some(message_id.as_str()) {
                    state.highlight_message = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The message a `Message` search hit points at: newest user/assistant/
    /// project message under the space whose prose contains `query`
    /// (already normalized). Name / title / member hits carry no message.
    fn find_message_match(&self, project_id: &str, query: &str) -> Option<(Id, Id)> {
        let space = self.project(project_id)?;
        self.space_search_hit(space, query)?.message
    }

    /// Every user/assistant/project message with prose under the space, as
    /// the search module reads them (system lines never count).
    fn searchable_messages(&self, project_id: &str) -> Vec<search::MessageText> {
        let mut out = Vec::new();
        for chat in self.tabs(project_id) {
            for m in self.messages_of(&chat.id) {
                match m.role {
                    Role::User | Role::Assistant | Role::Project => {}
                    Role::System => continue,
                }
                let text = crate::ui::sidebar::plain(&m.plain_text());
                if text.is_empty() {
                    continue;
                }
                out.push(search::MessageText {
                    chat_id: chat.id.clone(),
                    message_id: m.id.clone(),
                    created_at: m.created_at,
                    text,
                });
            }
        }
        out
    }

    /// How the sidebar row for `space` matches `query` (`search::normalize_query`
    /// output): the person's display name / @handle, the title, a member, a
    /// chat title, or a loaded message — `None` when it does not match at all.
    pub fn space_search_hit(&self, space: &Project, query: &str) -> Option<search::SearchHit> {
        let me = self.me_handle();
        search::space_hit(
            space,
            &me,
            self.tabs(&space.id)
                .iter()
                .filter(|c| !c.is_system)
                .map(|c| (c.id.as_str(), c.title.as_str())),
            || self.searchable_messages(&space.id),
            query,
        )
    }

    /// Everything the command palette can find for `query` (already
    /// `search::normalize_query`'d; empty = recents), app-wide whatever chat is
    /// open ([feedback search-palette-2026-10-24], [feedback
    /// search-palette-app-wide-2026-10-24]): every chat row through the #1477
    /// matcher (name, @handle, title, member, chat title — never a message
    /// here), people met in groups and shared projects who have no DM yet,
    /// servers and their channels by name, and — from the local
    /// [`SearchIndex`] — message text, attachments and links in every chat
    /// doc on this Mac. Ordering and grouping is `palette::arrange`.
    pub fn palette_entries(&self, query: &str) -> Vec<palette::Entry> {
        use crate::palette::{Entry, Kind, Rank, Target};
        let me = self.me_handle();
        let mut out = Vec::new();
        for space in &self.projects {
            let Some(hit) = search::space_hit(
                space,
                &me,
                self.tabs(&space.id)
                    .iter()
                    .filter(|c| !c.is_system)
                    .map(|c| (c.id.as_str(), c.title.as_str())),
                Vec::new,
                query,
            ) else {
                continue;
            };
            let when = crate::ui::sidebar::last_activity(self, space);
            out.push(palette::space_entry(space, hit, when, query));
        }
        if !query.is_empty() {
            // People without a DM: members of my groups and shared projects,
            // the other side's owner, the last directory search. Opening one
            // starts the DM (`start_dm`).
            let dm_handles: HashSet<String> = self
                .people_spaces()
                .filter(|p| !p.is_people_group())
                .filter_map(|p| p.person().map(|x| x.handle.clone()))
                .collect();
            let mut seen: HashSet<String> = HashSet::new();
            let mut met: Vec<(Person, String)> = Vec::new();
            for space in &self.projects {
                let where_ = space.name.clone();
                for p in space.people.iter().chain(space.owner_person.iter()) {
                    if p.handle.is_empty() || p.handle == me || dm_handles.contains(&p.handle) {
                        continue;
                    }
                    if seen.insert(p.handle.clone()) {
                        met.push((p.clone(), where_.clone()));
                    }
                }
            }
            for p in &self.people_results {
                if p.handle.is_empty() || p.handle == me || dm_handles.contains(&p.handle) {
                    continue;
                }
                if seen.insert(p.handle.clone()) {
                    met.push((p.clone(), String::new()));
                }
            }
            for (p, where_) in met {
                let name = p.display_name();
                let rank = Rank::of_entity(&name, &p.handle, None, query);
                if rank == Rank::Indirect {
                    continue;
                }
                let mut subtitle = format!("@{}", p.handle);
                if !where_.is_empty() {
                    subtitle.push_str(&format!(" · in {where_}"));
                }
                out.push(Entry {
                    kind: Kind::Person,
                    title: name,
                    subtitle,
                    when: 0,
                    rank,
                    target: Target::Person(p.handle.clone()),
                });
            }
        }
        for server in &self.servers {
            let when = self.server_last_activity(server);
            let rank = if query.is_empty() {
                Rank::EVERYTHING
            } else {
                Rank::of(&server.name, query)
            };
            if rank != Rank::Indirect {
                out.push(Entry {
                    kind: Kind::Server,
                    title: server.name.clone(),
                    subtitle: palette::server_subtitle(server.channels.len(), server.member_count),
                    when,
                    rank,
                    target: Target::Server(server.id.clone()),
                });
            }
            if query.is_empty() {
                continue;
            }
            for channel in &server.channels {
                let rank = Rank::of(&channel.name, query);
                if rank == Rank::Indirect {
                    continue;
                }
                let when = self
                    .channel_row(channel)
                    .map(|r| r.last_ts as u64)
                    .filter(|t| *t > 0)
                    .unwrap_or(when);
                out.push(Entry {
                    kind: Kind::Channel,
                    title: format!("#{}", channel.name),
                    subtitle: server.name.clone(),
                    when,
                    rank,
                    target: Target::Channel {
                        server_id: server.id.clone(),
                        channel_id: channel.id.clone(),
                    },
                });
            }
        }
        if query.is_empty() {
            return out;
        }
        // Message text, attachments and links from the local index. A hit's
        // chat is an agent / people chat (by chat id) or a channel (by room id).
        let cap = palette::SECTION_CAP * 2;
        for hit in self.search_index.messages(query, cap) {
            // A row hidden for me (someone I blocked) is not searchable either.
            if self
                .messages_of(&hit.chat_id)
                .iter()
                .find(|m| m.id == hit.message_id)
                .is_some_and(|m| self.hides_message(&hit.chat_id, m))
            {
                continue;
            }
            let Some((title, target)) = self.palette_place(&hit.chat_id, &hit.message_id) else {
                continue;
            };
            out.push(Entry {
                kind: Kind::Message,
                title,
                subtitle: format!("{} · {}", hit.sender, hit.snippet),
                when: hit.created_at,
                rank: Rank::Contains,
                target,
            });
        }
        for hit in self.search_index.attachments(query, cap) {
            let Some((where_, _)) = self.palette_place(&hit.chat_id, &hit.message_id) else {
                continue;
            };
            let kind = match hit.kind {
                AttachmentKind::Image | AttachmentKind::Video => Kind::Media,
                _ => Kind::File,
            };
            let mut meta = hit.kind.label().to_string();
            if !hit.size_label.is_empty() {
                meta.push_str(" · ");
                meta.push_str(&hit.size_label);
            }
            out.push(Entry {
                kind,
                title: hit.name,
                subtitle: format!("{meta} · {} in {where_}", hit.sender),
                when: hit.created_at,
                rank: Rank::Contains,
                target: Target::Media {
                    chat_id: hit.chat_id,
                    message_id: hit.message_id,
                    part_id: hit.part_id,
                },
            });
        }
        for hit in self.search_index.links(query, cap) {
            let Some((where_, _)) = self.palette_place(&hit.chat_id, &hit.message_id) else {
                continue;
            };
            out.push(Entry {
                kind: Kind::Link,
                title: hit
                    .url
                    .trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .to_string(),
                subtitle: format!(
                    "{} · {} in {where_}",
                    palette::link_host(&hit.url),
                    hit.sender
                ),
                when: hit.created_at,
                rank: Rank::Contains,
                target: Target::Link(hit.url),
            });
        }
        if !query.is_empty() {
            out.extend(palette::settings_entries(query, &self.palette_agent_rows()));
        }
        out
    }

    /// Where an indexed message lives, for the palette: the chat's display
    /// name (the person for a DM, the agent / group otherwise, `#channel ·
    /// Server` for a channel room) and the target that opens it there.
    fn palette_place(&self, chat_id: &str, message_id: &str) -> Option<(String, palette::Target)> {
        use crate::palette::Target;
        if let Some(space) = self.project_of_chat(chat_id) {
            let name = match space
                .person()
                .filter(|_| space.is_people() && !space.is_people_group())
            {
                Some(p) => p.display_name(),
                None => space.name.clone(),
            };
            return Some((
                name,
                Target::Message {
                    space_id: space.id.clone(),
                    chat_id: chat_id.to_string(),
                    message_id: message_id.to_string(),
                },
            ));
        }
        for server in &self.servers {
            for channel in &server.channels {
                let room = channel
                    .room_id
                    .clone()
                    .or_else(|| self.channel_row(channel).map(|r| r.room_id.clone()));
                if room.as_deref() == Some(chat_id) {
                    return Some((
                        format!("#{} · {}", channel.name, server.name),
                        Target::ChannelMessage {
                            server_id: server.id.clone(),
                            channel_id: channel.id.clone(),
                            message_id: message_id.to_string(),
                        },
                    ));
                }
            }
        }
        None
    }

    /// Re-index one chat doc for the palette (a snapshot, a channel page).
    fn index_chat(&mut self, chat_id: &str) {
        let mut index = std::mem::take(&mut self.search_index);
        let messages = self.messages_of(chat_id);
        index.replace_chat(chat_id, messages, self);
        self.search_index = index;
    }

    /// Re-index one message for the palette (an upsert, a streaming delta).
    fn index_message(&mut self, chat_id: &str, message: &Message) {
        let mut index = std::mem::take(&mut self.search_index);
        index.upsert(chat_id, message, self);
        self.search_index = index;
    }

    /// Open what a palette row points at ([feedback search-palette-2026-10-24]).
    pub fn open_palette_target(&mut self, target: palette::Target, cx: &mut Context<Self>) {
        use crate::palette::Target;
        match target {
            Target::Space(space_id) => {
                self.open_server = None;
                self.select_project(space_id, cx);
            }
            Target::Message {
                space_id,
                chat_id,
                message_id,
            } => {
                self.open_server = None;
                self.reveal_search_message(space_id, chat_id, message_id, cx);
            }
            Target::Server(server_id) => self.enter_server(server_id, cx),
            Target::Channel {
                server_id,
                channel_id,
            } => {
                self.open_server = Some(server_id.clone());
                self.select_channel(server_id, channel_id, cx);
            }
            Target::ChannelMessage {
                server_id,
                channel_id,
                message_id,
            } => {
                self.open_server = Some(server_id.clone());
                self.select_channel(server_id.clone(), channel_id.clone(), cx);
                if let Some(room) = self
                    .server(&server_id)
                    .and_then(|s| s.channel(&channel_id))
                    .and_then(|c| self.room_of_channel(c))
                {
                    self.highlight_message = Some(message_id.clone());
                    self.reveal_message = Some((room, message_id));
                    self.reveal_nonce += 1;
                    cx.notify();
                }
            }
            Target::Person(handle) => self.start_dm(handle, cx),
            Target::Media {
                chat_id,
                message_id,
                part_id,
            } => self.open_media_in_chat(&chat_id, &message_id, &part_id, cx),
            Target::Link(url) => cx.open_url(&url),
            // The shell opens Settings rows itself (`Shell::open_setting`): the
            // pages live on the shell, not here.
            Target::Setting(_) => {}
        }
    }

    /// My owned agents as the palette's Settings rows need them: the open
    /// chat's agent first, then by activity ([feedback search-settings-mac-2026-10-26]).
    fn palette_agent_rows(&self) -> Vec<palette::AgentRow> {
        let mut agents: Vec<&Project> = self
            .projects
            .iter()
            .filter(|p| !p.is_people() && !p.is_guest() && p.archived_at.is_none())
            .collect();
        agents.sort_by_key(|p| {
            (
                self.selected_project.as_deref() != Some(p.id.as_str()),
                std::cmp::Reverse(crate::ui::sidebar::last_activity(self, p)),
            )
        });
        agents
            .into_iter()
            .map(|p| palette::AgentRow {
                project_id: p.id.clone(),
                name: p.name.clone(),
                model: if p.model.is_empty() {
                    "default".to_string()
                } else {
                    p.model.clone()
                },
                reasoning: p.effort.label().to_string(),
                voice: crate::ui::voicepicker::current_label(p, &self.voices),
            })
            .collect()
    }

    /// Open `chat_id` under `project_id` scrolled to `message_id` (a palette
    /// row that matched a message body); shares the home-search path.
    fn reveal_search_message(
        &mut self,
        project_id: Id,
        chat_id: Id,
        message_id: Id,
        cx: &mut Context<Self>,
    ) {
        self.search_chat = Some(chat_id.clone());
        // An agent's previous chat comes back onto its tab strip; a people chat is a sidebar
        // row, and opening one from Hidden chats or a search hit must leave it hidden.
        let people = self.project(&project_id).is_some_and(|p| p.is_people());
        if !people && self.archived_chats.remove(&chat_id) {
            self.command(ClientCommand::ArchiveChat {
                chat_id: chat_id.clone(),
                archived: false,
            });
        }
        self.selected_channel = None;
        self.selected_project = Some(project_id.clone());
        self.sidebar_overlay = false;
        self.active_tab.insert(project_id.clone(), chat_id.clone());
        self.last_chats.insert(project_id, chat_id.clone());
        if let Some(chat) = self
            .chats
            .values_mut()
            .flat_map(|v| v.iter_mut())
            .find(|c| c.id == chat_id)
        {
            chat.unread = false;
            chat.unread_count = 0;
        }
        self.command(ClientCommand::OpenChat {
            chat_id: chat_id.clone(),
        });
        self.highlight_message = Some(message_id.clone());
        self.reveal_message = Some((chat_id, message_id));
        self.reveal_nonce += 1;
        self.sync_focus();
        cx.notify();
    }

    pub fn select_tab(&mut self, chat_id: Id, cx: &mut Context<Self>) {
        let Some(project_id) = self.selected_project.clone() else {
            return;
        };
        if self.search_chat.as_deref() != Some(chat_id.as_str()) {
            self.search_chat = None;
        }
        if self.tasks_open.as_deref() != Some(chat_id.as_str()) {
            self.tasks_open = None;
        }
        self.reveal_message = None;
        self.highlight_message = None;
        // Single-chat: an agent has one chat, whichever one was asked for.
        let chat_id = if self
            .visible_tabs(&project_id)
            .iter()
            .any(|c| c.id == chat_id)
            || self.search_chat.as_deref() == Some(chat_id.as_str())
        {
            chat_id
        } else {
            match self.primary_chat(&project_id) {
                Some(primary) if !self.multi_chat => primary.id.clone(),
                _ => chat_id,
            }
        };
        self.active_tab.insert(project_id.clone(), chat_id.clone());
        self.last_chats.insert(project_id, chat_id.clone());
        if let Some(chat) = self
            .chats
            .values_mut()
            .flat_map(|v| v.iter_mut())
            .find(|c| c.id == chat_id)
        {
            chat.unread = false;
            chat.unread_count = 0;
        }
        self.sync_badge();
        // Reading it here clears its banners here too (the relay tells the other devices).
        notify::withdraw_chat(&chat_id);
        // A job tab is client-side (`jobtabs`): nothing for the relay to open or mark.
        if !crate::jobtabs::is_job_chat(&chat_id) {
            self.command(ClientCommand::MarkRead {
                chat_id: chat_id.clone(),
            });
            self.command(ClientCommand::OpenChat { chat_id });
        }
        self.sync_focus();
        cx.notify();
    }

    /// Can the open space take a new tab (⌘T, the pill's "+", the dropdown's
    /// New tab)? Only an agent's own chats: people DMs and groups and server
    /// channels have no project chats, so there the shortcut is a no-op and
    /// the controls are absent (Jacob 2026-10-23: "New chat failed: HTTP 404
    /// no such project" from ⌘T in a group).
    pub fn can_new_tab(&self) -> bool {
        new_tab_allowed(
            self.multi_chat,
            self.selected_project
                .as_deref()
                .and_then(|id| self.project(id)),
            self.selected_channel.is_some(),
        )
    }

    pub fn new_tab(&mut self, cx: &mut Context<Self>) {
        if !self.can_new_tab() {
            return;
        }
        let Some(project_id) = self.selected_project.clone() else {
            return;
        };
        // Reuse a never-used empty "New chat" (open or archived) so ⌘T / "+"
        // cannot pile identical empties in Previous chats.
        if let Some(existing) = self.reusable_new_chat(&project_id) {
            if self.archived_chats.contains(&existing) {
                self.unarchive_chat(existing, cx);
            } else {
                self.select_tab(existing, cx);
            }
            return;
        }
        let chat_id = new_id();
        // After the last tab, where the strip's "+" sits (the shell scrolls it
        // into view).
        self.chats
            .entry(project_id.clone())
            .or_default()
            .push(Chat {
                id: chat_id.clone(),
                project_id: project_id.clone(),
                title: "New chat".into(),
                is_pinned: false,
                is_system: false,
                kind: ChatKind::Chat,
                participants: Vec::new(),
                unread: false,
                unread_count: 0,
                last_message_at: now_ms(),
                archived_at: None,
            });
        self.messages.insert(chat_id.clone(), Vec::new());
        self.chat_ui.insert(chat_id.clone(), ChatUi::default());
        self.active_tab.insert(project_id.clone(), chat_id.clone());
        self.last_chats.insert(project_id.clone(), chat_id.clone());
        self.command(ClientCommand::NewChat {
            project_id,
            chat_id: chat_id.clone(),
        });
        self.command(ClientCommand::OpenChat { chat_id });
        cx.notify();
    }

    /// The tab's "×" / ⌘W: archives the chat (as the iPhone does). Returns
    /// false for the pinned "Main" chat, which never leaves the strip.
    pub fn close_tab(&mut self, chat_id: Id, cx: &mut Context<Self>) -> bool {
        if !self.multi_chat {
            return false;
        }
        if crate::jobtabs::is_job_chat(&chat_id) {
            return self.close_job_tab(&chat_id, cx);
        }
        let Some(project_id) = self.project_of_chat(&chat_id).map(|p| p.id.clone()) else {
            return false;
        };
        if self
            .tabs(&project_id)
            .iter()
            .any(|c| c.id == chat_id && c.is_pinned)
        {
            return false;
        }
        self.archive_chat(chat_id, cx);
        true
    }

    /// Double-click on a chat tab: set its title (registry + doc `retitle`).
    pub fn retitle_chat(&mut self, chat_id: Id, title: String, cx: &mut Context<Self>) {
        let title = title.trim().chars().take(34).collect::<String>();
        if title.is_empty() {
            return;
        }
        let Some(project_id) = self.project_of_chat(&chat_id).map(|p| p.id.clone()) else {
            return;
        };
        let Some(chat) = self
            .chats
            .values_mut()
            .flat_map(|v| v.iter_mut())
            .find(|c| c.id == chat_id)
        else {
            return;
        };
        if chat.title == title {
            return;
        }
        chat.title = title.clone();
        self.command(ClientCommand::Retitle {
            project_id,
            chat_id,
            title,
        });
        cx.notify();
    }

    /// A send from the composer. `mentions` are the member ids of its
    /// committed mention chips (#1596); the source unions them with what the
    /// text says.
    pub fn send(
        &mut self,
        text: String,
        attachments: Vec<PathBuf>,
        reply_to: Option<String>,
        mentions: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        self.send_with(text, attachments, None, reply_to, false, mentions, cx);
    }

    // ----- emoji / stickers / GIFs picker (ARB-544) --------------------------

    /// One tap in the picker: the sticker or GIF goes out as its own message at once
    /// (Jacob: "tap sends immediately"). The row shows at once — a sticker from the bytes
    /// the picker already holds, a GIF through the relay pass-through — and the server's
    /// frame with the same id replaces it.
    pub fn send_media(&mut self, pick: MediaPick, cx: &mut Context<Self>) {
        let Some(chat_id) = self.active_chat_id() else {
            return;
        };
        if crate::jobtabs::is_job_chat(&chat_id) || self.selected_channel.is_some() {
            return;
        }
        if crate::draft_dm::is_draft(&chat_id) {
            self.push_notice("Say hi first. Stickers and GIFs work once the chat has started.");
            cx.notify();
            return;
        }
        self.send_errors.remove(&chat_id);
        let message_id = new_id();
        let reply_to = {
            let ui = self.chat_ui.entry(chat_id.clone()).or_default();
            ui.reply_ref = None;
            ui.editing = None;
            ui.reply_to.take()
        };
        let att_id = new_id();
        let attachment = match &pick {
            MediaPick::Sticker { pack, item, bytes } => {
                let key = format!("local:{att_id}");
                if let Some((image, trimmed)) = decode_image_pair(bytes) {
                    self.thumbnails.insert(key.clone(), Arc::new(image));
                    if let Some(t) = trimmed {
                        self.thumbnails_trimmed.insert(key.clone(), Arc::new(t));
                    }
                }
                self.thumbnails_requested.insert(key.clone());
                Attachment {
                    name: format!(
                        "{}.{}",
                        item.id,
                        if item.mime == "image/webp" {
                            "webp"
                        } else {
                            "png"
                        }
                    ),
                    mime: item.mime.clone(),
                    blob_ref: Some(key),
                    bytes: Some(bytes.len() as u64),
                    kind: Some(AttachmentKind::Sticker),
                    title: None,
                    chart_part: None,
                    url: None,
                    source: Some(Box::new(AttachmentSource {
                        provider: pack.name.clone(),
                        id: format!("{}/{}", pack.id, item.id),
                        page_url: pack.page_url.clone(),
                    })),
                }
            }
            MediaPick::Gif(gif) => {
                let attachment = Attachment {
                    name: format!("{}.gif", gif.id),
                    mime: "image/gif".into(),
                    blob_ref: None,
                    bytes: None,
                    kind: Some(AttachmentKind::Image),
                    title: None,
                    chart_part: None,
                    url: Some(gif.url.clone()),
                    source: Some(Box::new(AttachmentSource {
                        provider: "KLIPY".into(),
                        id: gif.id.clone(),
                        page_url: gif.page_url.clone(),
                    })),
                };
                if let Some(key) = attachment.preview_key() {
                    self.request_fun_thumbnail(key);
                }
                attachment
            }
        };
        let parts = vec![
            Part::Attachment {
                id: att_id,
                attachment,
            },
            Part::Text {
                id: new_id(),
                text: String::new(),
            },
        ];
        self.messages
            .entry(chat_id.clone())
            .or_default()
            .push(Message {
                id: message_id.clone(),
                chat_id: chat_id.clone(),
                role: Role::User,
                sender: "user".into(),
                status: MessageStatus::Queued,
                turn: None,
                decision: None,
                subagent_id: None,
                subagent_name: None,
                handoff_messages: None,
                notice: None,
                worker: false,
                job: None,
                hidden: false,
                agent_reply_to: None,
                reactions: Default::default(),
                in_reply_to: None,
                routine_name: None,
                sender_name: None,
                sender_icon: None,
                sender_handle: None,
                sender_avatar: None,
                sender_avatar_color: None,
                mentions: Vec::new(),
                reply_to: reply_to.clone(),
                forwarded_from: None,
                reply_ref: None,
                asked_by_handle: None,
                owner_handle: None,
                usage: None,
                return_contract: None,
                waiting_user: false,
                waiting_approval: false,
                model: None,
                effort: None,
                created_at: now_ms(),
                completed_at: None,
                call_id: None,
                activity: None,
                pending: true,
                parts,
                hidden_parts: Vec::new(),
                greeting: false,
                preview: None,
            });
        if let MediaPick::Gif(gif) = &pick {
            let query = self
                .gif_results
                .as_ref()
                .map(|(q, _)| q.clone())
                .unwrap_or_default();
            self.command(ClientCommand::GifShare {
                id: gif.id.clone(),
                query,
            });
        }
        self.command(ClientCommand::SendMedia {
            chat_id: chat_id.clone(),
            message_id,
            pick,
            reply_to,
        });
        cx.emit(AppEvent::TranscriptChanged { chat_id });
        cx.notify();
    }

    /// Picker: ask the relay for GIFs matching `query` (empty = trending). The answer lands
    /// in `gif_results`; an answer for an older query is dropped.
    pub fn gif_search(&mut self, query: String, cx: &mut Context<Self>) {
        let query = query.trim().to_owned();
        if self
            .gif_results
            .as_ref()
            .is_some_and(|(q, page)| *q == query && !matches!(page, GifPage::Failed))
        {
            return;
        }
        if self.gif_query_pending.as_deref() == Some(query.as_str()) {
            return;
        }
        self.gif_query_pending = Some(query.clone());
        self.gif_results = None;
        self.command(ClientCommand::GifSearch { query, pos: None });
        cx.notify();
    }

    /// Picker: the next page of the current GIF results, if there is one.
    pub fn gif_more(&mut self, cx: &mut Context<Self>) {
        let Some((
            query,
            GifPage::Items {
                next: Some(pos), ..
            },
        )) = self.gif_results.clone()
        else {
            return;
        };
        if self.gif_query_pending.is_some() {
            return;
        }
        self.gif_query_pending = Some(query.clone());
        self.command(ClientCommand::GifSearch {
            query,
            pos: Some(pos),
        });
        cx.notify();
    }

    fn gif_results_arrived(&mut self, query: String, pos: Option<String>, page: GifPage) {
        if self.gif_query_pending.as_deref() != Some(query.as_str()) {
            return;
        }
        self.gif_query_pending = None;
        match (pos, self.gif_results.take()) {
            (Some(_), Some((q, GifPage::Items { mut items, next: _ }))) if q == query => {
                if let GifPage::Items { items: more, next } = page {
                    items.extend(more);
                    self.gif_results = Some((query, GifPage::Items { items, next }));
                } else {
                    self.gif_results = Some((query, GifPage::Items { items, next: None }));
                }
            }
            _ => self.gif_results = Some((query, page)),
        }
        if let Some((_, GifPage::Items { items, .. })) = &self.gif_results {
            let keys: Vec<String> = items
                .iter()
                .map(|g| format!("{}{}", crate::model::URL_KEY_PREFIX, g.preview.url))
                .collect();
            for key in keys {
                self.request_fun_thumbnail(key);
            }
        }
    }

    /// Picker: fetch the sticker-pack manifest once per session.
    pub fn ensure_sticker_packs(&mut self, cx: &mut Context<Self>) {
        if self.sticker_packs_requested {
            return;
        }
        self.sticker_packs_requested = true;
        self.command(ClientCommand::StickerPacks);
        cx.notify();
    }

    /// Picker: fetch a sticker's or a GIF preview's bytes into the thumbnail cache
    /// (`sticker:<pack>/<id>` / `url:<url>` keys; the sync layer routes them).
    pub fn request_fun_thumbnail(&mut self, key: String) {
        if !self.thumbnails_requested.insert(key.clone()) {
            return;
        }
        self.command(ClientCommand::FetchThumbnail {
            project_id: self.selected_project.clone().unwrap_or_default(),
            key,
        });
    }

    /// Picker: the raw bytes of a fetched sticker (`sticker:<pack>/<id>`), for a send.
    pub fn sticker_bytes(&self, key: &str) -> Option<Arc<Vec<u8>>> {
        self.sticker_raw.get(key).cloned()
    }

    /// Guide: inject `text` into the agent's running turn instead of queuing a
    /// follow-up; `mentions` as for [`Self::send`].
    pub fn guide(
        &mut self,
        text: String,
        reply_to: Option<String>,
        mentions: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        self.send_with(text, Vec::new(), None, reply_to, true, mentions, cx);
    }

    /// F11: a spoken message; `wav` is kept on the row as an `audio` attachment.
    pub fn send_voice(&mut self, text: String, wav: Arc<Vec<u8>>, cx: &mut Context<Self>) {
        self.send_with(text, Vec::new(), Some(wav), None, false, Vec::new(), cx);
    }

    /// Hover reply: the next send in this chat answers `message_id`.
    pub fn set_reply_to(&mut self, chat_id: Id, message_id: Id, cx: &mut Context<Self>) {
        let ui = self.chat_ui.entry(chat_id).or_default();
        ui.reply_to = Some(message_id);
        ui.reply_ref = None;
        ui.editing = None;
        cx.notify();
    }

    /// Clear the reply chip above the composer (in-chat or cross-chat).
    pub fn clear_reply_to(&mut self, chat_id: &str, cx: &mut Context<Self>) {
        if let Some(ui) = self.chat_ui.get_mut(chat_id) {
            ui.reply_to = None;
            ui.reply_ref = None;
        }
        cx.notify();
    }

    /// Bubble menu → Edit: put this own message's text in the composer.
    pub fn begin_edit(&mut self, chat_id: Id, message_id: Id, cx: &mut Context<Self>) {
        let ui = self.chat_ui.entry(chat_id).or_default();
        ui.editing = Some(message_id);
        ui.reply_to = None;
        ui.reply_ref = None;
        cx.notify();
    }

    /// Clear the Editing chip above the composer.
    pub fn clear_editing(&mut self, chat_id: &str, cx: &mut Context<Self>) {
        if let Some(ui) = self.chat_ui.get_mut(chat_id) {
            ui.editing = None;
        }
        cx.notify();
    }

    /// The cross-chat reference for `message_id` in `chat_id`: its address plus
    /// a snapshot of who wrote it and what it said (crdt-sync §6.2 `replyRef`).
    /// `None` when the row is not loaded.
    pub fn reply_ref_for(&self, chat_id: &str, message_id: &str) -> Option<ReplyRef> {
        let m = self
            .messages_of(chat_id)
            .iter()
            .find(|m| m.id == message_id)?;
        let space_id = self.space_of_chat(chat_id);
        let space = self.project(&space_id);
        let project_id = match space {
            Some(p) if p.is_people() => "people".to_owned(),
            Some(p) => p.id.clone(),
            None => space_id.clone(),
        };
        let mine = m.sender == "user" || m.sender == format!("user:{}", self.me_id());
        let (sender_name, sender_handle) = if mine {
            (self.me_name(), self.me_handle())
        } else if m.role == Role::User {
            (
                m.sender_name.clone().unwrap_or_default(),
                m.sender_handle.clone().unwrap_or_default(),
            )
        } else {
            // An agent's row: the Project's name, or the handle it signs with.
            let name = space
                .filter(|p| !p.is_people())
                .map(|p| p.name.clone())
                .or_else(|| m.sender_name.clone())
                .unwrap_or_default();
            (name, m.sender_handle.clone().unwrap_or_default())
        };
        let text = m.plain_text();
        let text = if text.trim().is_empty() {
            m.parts
                .iter()
                .find_map(|p| match p {
                    Part::Attachment { attachment, .. } => Some(attachment.name.clone()),
                    _ => None,
                })
                .unwrap_or_default()
        } else {
            text
        };
        Some(ReplyRef {
            chat_id: chat_id.to_owned(),
            project_id,
            message_id: message_id.to_owned(),
            chat_title: self.chat_display_title(chat_id),
            sender: m.sender.clone(),
            sender_name,
            sender_handle,
            text: ReplyRef::snapshot_text(&text),
        })
    }

    /// What a quote header calls `chat_id`: the group / DM / project name, or the
    /// tab title when the space has several chats.
    pub fn chat_display_title(&self, chat_id: &str) -> String {
        let space_id = self.space_of_chat(chat_id);
        if let Some(space) = self.project(&space_id) {
            if let Some(person) = space.person().filter(|_| space.kind == SpaceKind::Dm) {
                return person.display_name();
            }
            let chats = self.tabs(&space_id);
            let plain = chats
                .iter()
                .filter(|c| c.kind != ChatKind::Projects)
                .count();
            if plain > 1 {
                if let Some(c) = chats.iter().find(|c| c.id == chat_id) {
                    if c.is_group() || !c.is_pinned {
                        return c.title.clone();
                    }
                }
            }
            return space.name.clone();
        }
        self.chats
            .values()
            .flat_map(|v| v.iter())
            .find(|c| c.id == chat_id)
            .map(|c| c.title.clone())
            .unwrap_or_default()
    }

    /// Is `chat_id` one of this user's chats (people chat, own or shared
    /// project)? Only then can a cross-chat quote open its original.
    pub fn is_member_of_chat(&self, chat_id: &str) -> bool {
        self.chats
            .values()
            .chain(self.job_tabs.values())
            .flat_map(|v| v.iter())
            .any(|c| c.id == chat_id)
            || self.project(chat_id).is_some_and(|p| p.is_people())
    }

    /// Bubble menu → Reply in DM: open (or create) the DM with the person who
    /// wrote `message_id` and arm the composer there with a cross-chat reply
    /// to it. Nothing happens for my own rows or an agent's (no peer to DM).
    pub fn reply_in_dm(&mut self, chat_id: Id, message_id: Id, cx: &mut Context<Self>) {
        let Some(reply_ref) = self.reply_ref_for(&chat_id, &message_id) else {
            return;
        };
        let me = format!("user:{}", self.me_id());
        if !reply_ref.sender.starts_with("user:")
            || reply_ref.sender == me
            || reply_ref.sender_handle.is_empty()
        {
            return;
        }
        self.pending_dm_reply = Some(reply_ref.clone());
        self.start_dm(reply_ref.sender_handle, cx);
        // The DM already exists: `start_dm` selected it synchronously.
        self.arm_pending_dm_reply(cx);
    }

    /// Once the DM is the open space, move the pending cross-chat reply onto
    /// its composer. Called when `start_dm` selects an existing DM at once and
    /// when a freshly created one arrives from the relay.
    fn arm_pending_dm_reply(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.pending_dm_reply.clone() else {
            return;
        };
        let Some(space) = self
            .selected_project
            .as_deref()
            .and_then(|id| self.project(id))
        else {
            return;
        };
        let is_dm = space.kind == SpaceKind::Dm
            && space.person().is_some_and(|p| p.handle == r.sender_handle);
        if !is_dm {
            return;
        }
        let Some(dm_chat) = self.active_chat_id() else {
            return;
        };
        self.pending_dm_reply = None;
        let ui = self.chat_ui.entry(dm_chat).or_default();
        ui.reply_to = None;
        ui.editing = None;
        ui.reply_ref = Some(r);
        cx.notify();
    }

    /// Click on a cross-chat quote: open the source chat scrolled to the
    /// original and highlight it. `false` when the viewer is not in that chat.
    pub fn reveal_reply_ref(&mut self, r: &ReplyRef, cx: &mut Context<Self>) -> bool {
        if !self.is_member_of_chat(&r.chat_id) {
            return false;
        }
        let space_id = self.space_of_chat(&r.chat_id);
        self.close_profile(cx);
        self.reveal_search_message(space_id, r.chat_id.clone(), r.message_id.clone(), cx);
        self.highlight_message = Some(r.message_id.clone());
        let message_id = r.message_id.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(QUOTE_HIGHLIGHT_MS))
                .await;
            let _ = this.update(cx, |state, cx| {
                if state.highlight_message.as_deref() == Some(message_id.as_str()) {
                    state.highlight_message = None;
                    cx.notify();
                }
            });
        })
        .detach();
        true
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn send_with(
        &mut self,
        text: String,
        attachments: Vec<PathBuf>,
        voice_wav: Option<Arc<Vec<u8>>>,
        reply_to: Option<String>,
        steer: bool,
        mentions: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(chat_id) = self.active_chat_id() else {
            return;
        };
        // A new send clears the composer's note (the next failure brings its own).
        self.send_errors.remove(&chat_id);
        if crate::jobtabs::is_job_chat(&chat_id) {
            self.steer_from_job_tab(&chat_id, text, cx);
            return;
        }
        if self.selected_channel.is_some() {
            self.send_to_channel(
                chat_id,
                text,
                attachments.len() + usize::from(voice_wav.is_some()),
                cx,
            );
            return;
        }
        let message_id = new_id();
        // Guide never answers a card — it steers the live turn.
        let answer = !steer && self.chat_needs_answer(&chat_id);
        let reply_ref = {
            let ui = self.chat_ui.entry(chat_id.clone()).or_default();
            if !ui.composer_dropped {
                ui.composer_dropped = true;
                ui.drop_started_at = Some(now_ms());
            }
            ui.reply_to = None;
            ui.editing = None;
            // A cross-chat reply never carries an in-chat `replyTo` as well.
            ui.reply_ref.take().filter(|_| reply_to.is_none())
        };
        // Title from the first message (§3.4); the source also queues a
        // `retitle` command for the box.
        if let Some(chat) = self
            .chats
            .values_mut()
            .flat_map(|v| v.iter_mut())
            .find(|c| c.id == chat_id && !c.is_pinned && c.title == "New chat")
        {
            chat.title = title_from(&text);
        }
        // Optimistic echo: the row shows at once; the source's frame with the
        // same id replaces it (§3.6). Image files get a local thumbnail so the
        // card shows a picture immediately, not "No preview".
        let mut parts: Vec<Part> = Vec::new();
        for path in &attachments {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("attachment")
                .to_string();
            let mime = mime_for_path(path).to_string();
            let att_id = new_id();
            let mut blob_ref = None;
            let bytes = if let Ok(data) = std::fs::read(path) {
                let probe = crate::model::Attachment {
                    name: name.clone(),
                    mime: mime.clone(),
                    blob_ref: None,
                    bytes: Some(data.len() as u64),
                    kind: None,
                    title: None,
                    chart_part: None,
                    url: None,
                    source: None,
                };
                if probe.is_image() {
                    if let Some((image, trimmed)) = decode_image_pair(&data) {
                        let key = format!("local:{att_id}");
                        self.thumbnails.insert(key.clone(), Arc::new(image));
                        if let Some(t) = trimmed {
                            self.thumbnails_trimmed.insert(key.clone(), Arc::new(t));
                        }
                        self.thumbnails_requested.insert(key.clone());
                        blob_ref = Some(key);
                    }
                }
                Some(data.len() as u64)
            } else {
                std::fs::metadata(path).ok().map(|m| m.len())
            };
            parts.push(Part::Attachment {
                id: att_id,
                attachment: crate::model::Attachment {
                    name,
                    mime,
                    blob_ref,
                    bytes,
                    kind: None,
                    title: None,
                    chart_part: None,
                    url: None,
                    source: None,
                },
            });
        }
        if voice_wav.is_some() {
            parts.push(Part::Attachment {
                id: new_id(),
                attachment: crate::model::Attachment {
                    name: "voice-memo.wav".into(),
                    mime: "audio/wav".into(),
                    blob_ref: None,
                    bytes: None,
                    kind: Some(AttachmentKind::Audio),
                    title: None,
                    chart_part: None,
                    url: None,
                    source: None,
                },
            });
        }
        parts.push(Part::Text {
            id: new_id(),
            text: text.clone(),
        });
        self.messages
            .entry(chat_id.clone())
            .or_default()
            .push(Message {
                id: message_id.clone(),
                chat_id: chat_id.clone(),
                role: Role::User,
                sender: "user".into(),
                // Guide lands in the live turn (not queued behind it).
                status: if steer {
                    MessageStatus::Done
                } else {
                    MessageStatus::Queued
                },
                turn: None,
                decision: None,
                subagent_id: None,
                subagent_name: None,
                handoff_messages: None,
                notice: None,
                worker: false,
                job: None,
                hidden: false,
                agent_reply_to: None,
                reactions: Default::default(),
                in_reply_to: None,
                routine_name: None,
                sender_name: None,
                sender_icon: None,
                sender_handle: None,
                sender_avatar: None,
                sender_avatar_color: None,
                mentions: mentions.clone(),
                reply_to: reply_to.clone(),
                forwarded_from: None,
                reply_ref: reply_ref.clone(),
                asked_by_handle: None,
                owner_handle: None,
                usage: None,
                return_contract: None,
                waiting_user: false,
                waiting_approval: false,
                model: None,
                effort: None,
                created_at: now_ms(),
                completed_at: if steer { Some(now_ms()) } else { None },
                call_id: None,
                activity: None,
                pending: true,
                parts,
                hidden_parts: Vec::new(),
                greeting: false,
                preview: None,
            });
        if answer {
            let question_id = self.open_plain_question_id(&chat_id);
            let value = question_id
                .as_ref()
                .map(|_| serde_json::Value::String(text.clone()));
            if let Some(qid) = &question_id {
                self.mark_question_answered(&chat_id, qid, value.clone());
            }
            self.command(ClientCommand::Answer {
                chat_id: chat_id.clone(),
                message_id,
                text,
                question_id,
                value,
            });
        } else if steer {
            self.command(ClientCommand::Steer {
                chat_id: chat_id.clone(),
                message_id,
                text,
                reply_to,
            });
        } else if crate::draft_dm::is_draft(&chat_id) {
            self.queue_draft_send(crate::draft_dm::DraftSend {
                text,
                attachments,
                voice_wav,
                reply_to,
                reply_ref,
                mentions,
            });
        } else if let Some(wav) = voice_wav {
            self.command(ClientCommand::SendVoice {
                chat_id: chat_id.clone(),
                message_id,
                text,
                wav,
            });
        } else {
            self.command(ClientCommand::Send {
                chat_id: chat_id.clone(),
                message_id,
                text,
                attachments,
                reply_to,
                reply_ref,
                mentions,
            });
        }
        cx.emit(AppEvent::TranscriptChanged { chat_id });
        cx.notify();
    }

    // ----- servers and channels (D88) --------------------------------------

    pub fn server(&self, server_id: &str) -> Option<&ServerInfo> {
        self.servers.iter().find(|s| s.id == server_id)
    }

    /// The server whose channel list the left pane shows.
    pub fn open_server_info(&self) -> Option<&ServerInfo> {
        self.open_server.as_deref().and_then(|id| self.server(id))
    }

    /// The channel open in the centre, with its server.
    pub fn selected_channel_info(&self) -> Option<(&ServerInfo, &crate::model::ServerChannel)> {
        let sel = self.selected_channel.as_ref()?;
        let server = self.server(&sel.server_id)?;
        let channel = server.channel(&sel.channel_id)?;
        Some((server, channel))
    }

    /// The homeserver's row for a channel (preview, unread, `can_post`), by registry id or room.
    pub fn channel_row(&self, channel: &crate::model::ServerChannel) -> Option<&ChannelRow> {
        self.matrix_snapshot.channels.iter().find(|c| {
            c.channel_id == channel.id || channel.room_id.as_deref().is_some_and(|r| r == c.room_id)
        })
    }

    /// The homeserver's row for a server (last activity across its channels, unread).
    pub fn space_row(&self, server: &ServerInfo) -> Option<&SpaceRow> {
        self.matrix_snapshot.spaces.iter().find(|s| {
            s.server_id == server.id
                || server
                    .space_room_id
                    .as_deref()
                    .is_some_and(|r| r == s.room_id)
        })
    }

    /// Last activity of a server for the left list's ordering (ms).
    pub fn server_last_activity(&self, server: &ServerInfo) -> u64 {
        let from_rooms = self
            .space_row(server)
            .map(|s| s.last_ts)
            .filter(|t| *t > 0)
            .map(|t| t as u64);
        from_rooms.unwrap_or(server.created_at.max(0) as u64)
    }

    /// Rule 9: `false` on a restricted channel where I am not an admin.
    pub fn channel_can_post(&self) -> bool {
        let Some((server, channel)) = self.selected_channel_info() else {
            return true;
        };
        if server.can_admin() || !channel.restricted {
            return true;
        }
        self.channel_row(channel).is_some_and(|r| r.can_post)
    }

    /// The room of a channel: the registry's, else the one the homeserver lists for its id.
    fn room_of_channel(&self, channel: &crate::model::ServerChannel) -> Option<String> {
        channel
            .room_id
            .clone()
            .or_else(|| self.channel_row(channel).map(|r| r.room_id.clone()))
    }

    /// Server row click: slide the left pane into its channels and land on `#general`.
    pub fn enter_server(&mut self, server_id: Id, cx: &mut Context<Self>) {
        let Some(server) = self.server(&server_id).cloned() else {
            return;
        };
        self.open_server = Some(server_id.clone());
        self.sidebar_overlay = false;
        if let Some(general) = server.general() {
            let id = general.id.clone();
            self.select_channel(server_id, id, cx);
        } else {
            cx.notify();
        }
    }

    /// A server's picture in the collapsed rail ([feedback mac-sidebar-rail-2026-10-26]):
    /// its channel list is one toggle away, so the click lands on the server's newest
    /// channel (last activity), `#general` when none has spoken, and marks the server
    /// open so expanding shows its channels.
    pub fn enter_server_latest(&mut self, server_id: Id, cx: &mut Context<Self>) {
        let Some(server) = self.server(&server_id).cloned() else {
            return;
        };
        let newest = self
            .matrix_snapshot
            .channels
            .iter()
            .filter(|c| c.server_id == server.id && c.last_ts > 0)
            .max_by_key(|c| c.last_ts)
            .and_then(|c| server.channels.iter().find(|ch| ch.id == c.channel_id))
            .or_else(|| server.general())
            .cloned();
        self.open_server = Some(server_id.clone());
        self.sidebar_overlay = false;
        match newest {
            Some(channel) => self.select_channel(server_id, channel.id.clone(), cx),
            None => cx.notify(),
        }
    }

    /// Back arrow at the top of the channel list: the chat list again. The channel in the
    /// centre stays until a chat is picked.
    pub fn close_server_pane(&mut self, cx: &mut Context<Self>) {
        if self.open_server.take().is_some() {
            cx.notify();
        }
    }

    pub fn select_channel(&mut self, server_id: Id, channel_id: Id, cx: &mut Context<Self>) {
        let Some(channel) = self
            .server(&server_id)
            .and_then(|s| s.channel(&channel_id))
            .cloned()
        else {
            return;
        };
        let room_id = self.room_of_channel(&channel);
        let same = self
            .selected_channel
            .as_ref()
            .is_some_and(|c| c.server_id == server_id && c.channel_id == channel_id);
        if !same {
            self.close_profile(cx);
        }
        self.search_chat = None;
        self.reveal_message = None;
        self.highlight_message = None;
        self.chat_search = None;
        self.renaming_project = None;
        self.selected_project = None;
        self.sidebar_overlay = false;
        self.selected_channel = Some(ChannelSel {
            server_id: server_id.clone(),
            channel_id: channel_id.clone(),
            room_id: room_id.clone(),
        });
        // Open only when Matrix already lists the room. A registry room_id alone can
        // race sliding sync (memory store empty until the next snapshot) or point at a
        // gone room — both yield "no such room", which is not a product bug.
        let synced = room_id.as_ref().is_some_and(|rid| {
            self.matrix_snapshot
                .channels
                .iter()
                .any(|c| c.room_id == *rid)
        });
        match room_id {
            Some(room_id) if synced => {
                self.channel_pending_room = None;
                self.command(ClientCommand::ChannelOpen {
                    room_id: room_id.clone(),
                });
                self.command(ClientCommand::ChannelMarkRead { room_id });
            }
            // Just created, or Matrix has not listed the room yet: wait for a snapshot.
            _ => self.channel_pending_room = Some((server_id, channel_id)),
        }
        self.sync_focus();
        cx.notify();
    }

    /// A channel whose room was unknown when it was opened: resolve it once it is listed.
    fn resolve_pending_channel(&mut self, cx: &mut Context<Self>) {
        let Some((server_id, channel_id)) = self.channel_pending_room.clone() else {
            return;
        };
        let still_open = self
            .selected_channel
            .as_ref()
            .is_some_and(|c| c.server_id == server_id && c.channel_id == channel_id);
        if !still_open {
            self.channel_pending_room = None;
            return;
        }
        let room = self
            .server(&server_id)
            .and_then(|s| s.channel(&channel_id))
            .and_then(|c| self.room_of_channel(c));
        let synced = room.as_ref().is_some_and(|rid| {
            self.matrix_snapshot
                .channels
                .iter()
                .any(|c| c.room_id == *rid)
        });
        if synced {
            self.select_channel(server_id, channel_id, cx);
        }
    }

    pub fn open_server_sheet(&mut self, sheet: ServerSheet, cx: &mut Context<Self>) {
        self.new_chat_open = false;
        self.new_group_open = false;
        self.new_project_open = false;
        self.server_error = None;
        self.server_busy = false;
        self.server_sheet = Some(sheet);
        cx.notify();
    }

    /// Closes the sheet; `true` when one was open.
    pub fn close_server_sheet(&mut self, cx: &mut Context<Self>) -> bool {
        let was = self.server_sheet.take().is_some();
        self.server_error = None;
        self.server_busy = false;
        if was {
            cx.notify();
        }
        was
    }

    /// Sheet "Create": `POST /servers`; the answer opens it on `#general`.
    pub fn create_server(
        &mut self,
        name: String,
        leaf: Option<crate::ui::avatars::LeafPick>,
        picture: Option<crate::sync::DraftPicture>,
        cx: &mut Context<Self>,
    ) {
        let name = name.trim().chars().take(64).collect::<String>();
        if name.is_empty() || self.server_busy {
            return;
        }
        self.server_busy = true;
        self.server_error = None;
        self.command(ClientCommand::ServerCreate {
            name,
            leaf,
            picture,
        });
        cx.notify();
    }

    /// `arbos://join/<code>` (the site's invite page): the Join sheet opens with the code.
    pub fn open_join_link(&mut self, code: String, cx: &mut Context<Self>) {
        if code.is_empty() {
            return;
        }
        self.pending_join_code = Some(code);
        self.open_server_sheet(ServerSheet::JoinServer, cx);
    }

    /// Sheet "Join": `POST /servers/join` with the pasted link or code.
    pub fn join_server(&mut self, code: String, cx: &mut Context<Self>) {
        let code = code.trim().to_string();
        if code.is_empty() || self.server_busy {
            return;
        }
        self.server_busy = true;
        self.server_error = None;
        self.command(ClientCommand::ServerJoin { code });
        cx.notify();
    }

    /// Rule 7: letters, digits and dashes; lower-cased, spaces become dashes.
    pub fn channel_name_clean(name: &str) -> String {
        let mut out = String::new();
        for ch in name.trim().chars().take(48) {
            if ch.is_ascii_alphanumeric() {
                out.extend(ch.to_lowercase());
            } else if (ch == '-' || ch.is_whitespace() || ch == '_') && !out.ends_with('-') {
                out.push('-');
            }
        }
        out.trim_matches('-').to_string()
    }

    /// Sheet "Create" inside a server (owner/admin): `POST /servers/{id}/channels`.
    pub fn create_channel(
        &mut self,
        server_id: Id,
        name: String,
        restricted: bool,
        cx: &mut Context<Self>,
    ) {
        let name = Self::channel_name_clean(&name);
        if name.is_empty() || self.server_busy {
            return;
        }
        if !self.server(&server_id).is_some_and(ServerInfo::can_admin) {
            return;
        }
        self.server_busy = true;
        self.server_error = None;
        self.command(ClientCommand::ChannelCreate {
            server_id,
            name,
            restricted,
        });
        cx.notify();
    }

    /// Server page: rename (owner/admin) — optimistic, then `PATCH /servers/{id}`.
    pub fn set_server_name(&mut self, server_id: Id, name: String, cx: &mut Context<Self>) {
        let name = name.trim().chars().take(64).collect::<String>();
        self.profile_editing_name = false;
        if name.is_empty() {
            cx.notify();
            return;
        }
        if let Some(s) = self.servers.iter_mut().find(|s| s.id == server_id) {
            if s.name == name || !s.can_admin() {
                cx.notify();
                return;
            }
            s.name = name.clone();
        }
        self.command(ClientCommand::ServerRename { server_id, name });
        cx.notify();
    }

    /// Server page, Channels tab: rename a channel (owner/admin).
    pub fn set_channel_name(
        &mut self,
        server_id: Id,
        channel_id: Id,
        name: String,
        cx: &mut Context<Self>,
    ) {
        let name = Self::channel_name_clean(&name);
        if name.is_empty() {
            cx.notify();
            return;
        }
        if let Some(c) = self
            .servers
            .iter_mut()
            .find(|s| s.id == server_id)
            .filter(|s| s.can_admin())
            .and_then(|s| s.channels.iter_mut().find(|c| c.id == channel_id))
        {
            if c.name == name {
                cx.notify();
                return;
            }
            c.name = name.clone();
        }
        self.command(ClientCommand::ChannelPatch {
            server_id,
            channel_id,
            name: Some(name),
            topic: None,
            restricted: None,
        });
        cx.notify();
    }

    /// Channel page, Settings: the topic (owner/admin; ≤ 200 chars, empty clears it).
    pub fn set_channel_topic(
        &mut self,
        server_id: Id,
        channel_id: Id,
        topic: String,
        cx: &mut Context<Self>,
    ) {
        let topic: String = topic.trim().chars().take(200).collect();
        if let Some(c) = self
            .servers
            .iter_mut()
            .find(|s| s.id == server_id)
            .filter(|s| s.can_admin())
            .and_then(|s| s.channels.iter_mut().find(|c| c.id == channel_id))
        {
            if c.topic == topic {
                cx.notify();
                return;
            }
            c.topic = topic.clone();
        }
        self.command(ClientCommand::ChannelPatch {
            server_id,
            channel_id,
            name: None,
            topic: Some(topic),
            restricted: None,
        });
        cx.notify();
    }

    /// Server page, Channels tab: "Admins only post" toggle (owner/admin).
    pub fn set_channel_restricted(
        &mut self,
        server_id: Id,
        channel_id: Id,
        restricted: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(c) = self
            .servers
            .iter_mut()
            .find(|s| s.id == server_id)
            .filter(|s| s.can_admin())
            .and_then(|s| s.channels.iter_mut().find(|c| c.id == channel_id))
        {
            c.restricted = restricted;
        }
        self.command(ClientCommand::ChannelPatch {
            server_id,
            channel_id,
            name: None,
            topic: None,
            restricted: Some(restricted),
        });
        cx.notify();
    }

    /// Channel row menu / server page: hide a channel from my list, or bring it back. An
    /// archived channel that was open gives way to `#general`.
    pub fn archive_channel(
        &mut self,
        server_id: Id,
        channel_id: Id,
        archived: bool,
        cx: &mut Context<Self>,
    ) {
        let was_open = self
            .selected_channel
            .as_ref()
            .is_some_and(|c| c.channel_id == channel_id);
        if let Some(c) = self
            .servers
            .iter_mut()
            .find(|s| s.id == server_id)
            .and_then(|s| s.channels.iter_mut().find(|c| c.id == channel_id))
        {
            c.archived = archived;
        }
        self.command(ClientCommand::ChannelArchive {
            server_id: server_id.clone(),
            channel_id: channel_id.clone(),
            archived,
        });
        if archived && was_open {
            match self
                .server(&server_id)
                .and_then(|s| s.general())
                .map(|g| g.id.clone())
                .filter(|g| *g != channel_id)
            {
                Some(general) => {
                    self.select_channel(server_id, general, cx);
                    return;
                }
                None => self.selected_channel = None,
            }
        }
        cx.notify();
    }

    /// Server page, Channels tab: delete a channel (owner/admin; `#general` refuses).
    pub fn delete_channel(&mut self, server_id: Id, channel_id: Id, cx: &mut Context<Self>) {
        let was_open = self
            .selected_channel
            .as_ref()
            .is_some_and(|c| c.channel_id == channel_id);
        if let Some(s) = self
            .servers
            .iter_mut()
            .find(|s| s.id == server_id)
            .filter(|s| s.can_admin())
        {
            s.channels.retain(|c| c.id != channel_id);
        }
        self.command(ClientCommand::ChannelDelete {
            server_id: server_id.clone(),
            channel_id,
        });
        if was_open {
            if let Some(general) = self.server(&server_id).and_then(|s| s.general()) {
                let id = general.id.clone();
                self.select_channel(server_id, id, cx);
                return;
            }
            self.selected_channel = None;
        }
        cx.notify();
    }

    /// Owner: make a member an admin (or an admin a member again).
    pub fn set_member_role(
        &mut self,
        server_id: Id,
        user_id: Id,
        role: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(m) = self
            .servers
            .iter_mut()
            .find(|s| s.id == server_id)
            .filter(|s| s.is_owner())
            .and_then(|s| s.members.iter_mut().find(|m| m.id == user_id))
        {
            m.role = role.to_string();
        }
        self.command(ClientCommand::ServerSetRole {
            server_id,
            user_id,
            role: role.to_string(),
        });
        cx.notify();
    }

    /// Owner/admin: remove a member; `ban` keeps them out.
    pub fn remove_server_member(
        &mut self,
        server_id: Id,
        user_id: Id,
        ban: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(s) = self
            .servers
            .iter_mut()
            .find(|s| s.id == server_id)
            .filter(|s| s.can_admin())
        {
            s.members.retain(|m| m.id != user_id);
            s.member_count = s.member_count.saturating_sub(1);
        }
        self.command(ClientCommand::ServerRemoveMember {
            server_id,
            user_id,
            ban,
        });
        cx.notify();
    }

    /// Settings › Invite links › "New link" (owner): with or without a 7-day expiry / use limit.
    pub fn create_invite(
        &mut self,
        server_id: Id,
        expires: bool,
        max_uses: Option<i64>,
        cx: &mut Context<Self>,
    ) {
        if !self.server(&server_id).is_some_and(ServerInfo::is_owner) {
            return;
        }
        self.command(ClientCommand::ServerInviteCreate {
            server_id,
            expires,
            max_uses,
        });
        cx.notify();
    }

    /// Settings › Invite links › ✕ (owner): the link stops working at once.
    pub fn revoke_invite(&mut self, server_id: Id, invite_id: Id, cx: &mut Context<Self>) {
        if let Some(s) = self
            .servers
            .iter_mut()
            .find(|s| s.id == server_id)
            .filter(|s| s.is_owner())
        {
            s.invites.retain(|i| i.id != invite_id);
        }
        self.command(ClientCommand::ServerInviteRevoke {
            server_id,
            invite_id,
        });
        cx.notify();
    }

    /// Member: leave; owner: delete. Either way the server leaves this Mac at once.
    pub fn leave_server(&mut self, server_id: Id, cx: &mut Context<Self>) {
        let owner = self.server(&server_id).is_some_and(ServerInfo::is_owner);
        self.command(if owner {
            ClientCommand::ServerDelete {
                server_id: server_id.clone(),
            }
        } else {
            ClientCommand::ServerLeave {
                server_id: server_id.clone(),
            }
        });
        self.drop_server(&server_id, cx);
    }

    /// Forget a server locally (left, deleted, or gone from the list).
    fn drop_server(&mut self, server_id: &str, cx: &mut Context<Self>) {
        self.servers.retain(|s| s.id != server_id);
        if self.open_server.as_deref() == Some(server_id) {
            self.open_server = None;
        }
        if self
            .selected_channel
            .as_ref()
            .is_some_and(|c| c.server_id == server_id)
        {
            self.selected_channel = None;
            self.channel_pending_room = None;
            self.sync_focus();
        }
        if matches!(
            &self.profile,
            Some(ProfilePage::Server { server_id: id }) | Some(ProfilePage::Channel { server_id: id, .. })
                if id == server_id
        ) {
            self.close_profile(cx);
        }
        cx.notify();
    }

    /// `user:<my id>`: the member string own channel rows carry (`crdt-sync.md` §6.2).
    fn me_member(&self) -> String {
        format!("user:{}", self.me_id())
    }

    /// One transcript row from a homeserver row: the JSON is already the chat-doc shape.
    fn channel_message(&self, room_id: &str, row: &MessageRow) -> Option<Message> {
        let mut m: Message = serde_json::from_str(&row.json).ok()?;
        m.chat_id = room_id.to_string();
        m.pending = false;
        if m.sender.is_empty() {
            m.sender = row.sender.clone();
        }
        let person = m.sender.starts_with("user:");
        // An agent's Matrix display name is its name; a person's is their handle, never
        // their name ([feedback server-member-names-2026-10-28]) — see `fill_channel_sender`.
        if !person
            && m.sender_name.as_deref().is_none_or(str::is_empty)
            && !row.sender_name.is_empty()
        {
            m.sender_name = Some(row.sender_name.clone());
        }
        if m.created_at == 0 && row.created_at > 0 {
            m.created_at = row.created_at as u64;
        }
        if m.sender_handle.as_deref().is_some_and(str::is_empty) {
            m.sender_handle = None;
        }
        // A homeserver row names its sender by Matrix localpart, which is lower-case; relay
        // ids keep their case (`test_<ULID>`). Own rows take the exact member string the
        // transcript's "mine" rule compares against, and the people-chat avatar lookup.
        if m.sender.eq_ignore_ascii_case(&self.me_member()) {
            m.sender = self.me_member();
            m.sender_handle = Some(self.me_handle());
        } else if person {
            self.fill_channel_sender(&mut m);
        }
        Some(m)
    }

    /// Who a channel row is from, as the rosters know them: the servers' member lists (by
    /// user id, then handle), then every people chat's members and the directory. A Matrix
    /// row only names its sender by handle (`arbos_matrix::message_row`), so this is where
    /// a channel bubble gets the display name a group bubble shows
    /// ([feedback server-member-names-2026-10-28]; Affinity, Arbonites 2026-10-28: "we are
    /// referred to as our handles instead of our usernames in server").
    pub fn channel_sender(&self, sender: &str, handle: Option<&str>) -> Option<Person> {
        let id = sender.strip_prefix("user:")?;
        let handle = handle.filter(|h| !h.is_empty());
        self.servers
            .iter()
            .flat_map(|s| s.members.iter())
            .find(|p| {
                (!p.id.is_empty() && p.id.eq_ignore_ascii_case(id))
                    || handle.is_some_and(|h| p.handle.eq_ignore_ascii_case(h))
            })
            .cloned()
            .or_else(|| handle.and_then(|h| self.person_by_handle(h)))
            .or_else(|| {
                self.people_directory
                    .iter()
                    .find(|p| p.id.eq_ignore_ascii_case(id))
                    .cloned()
            })
    }

    /// Fill a person's channel row from the roster: the display name (when the roster has
    /// one), the picture (when the row carries none) and the relay's spelling of the user id
    /// (the Matrix localpart is lower-case; the disc's gradient is seeded by the id, so a
    /// bubble and the Members row would otherwise draw two colours for one person). Returns
    /// whether anything changed.
    fn fill_channel_sender(&self, m: &mut Message) -> bool {
        let Some(p) = self.channel_sender(&m.sender, m.sender_handle.as_deref()) else {
            return false;
        };
        let mut changed = false;
        if !p.id.is_empty() {
            let member = format!("user:{}", p.id);
            if m.sender != member && m.sender.eq_ignore_ascii_case(&member) {
                m.sender = member;
                changed = true;
            }
        }
        if m.sender_name.as_deref().is_none_or(str::is_empty) {
            if let Some(name) = p.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                m.sender_name = Some(name.to_owned());
                changed = true;
            }
        }
        if m.sender_avatar.as_deref().is_none_or(str::is_empty) && !p.avatar_url.is_empty() {
            m.sender_avatar = Some(p.avatar_url.clone());
            changed = true;
        }
        changed
    }

    /// A roster arrived (server members, the list): name the channel rows it now knows.
    /// Decoded rows are kept, so a channel opened before its server's detail came back
    /// would otherwise keep reading handles.
    fn refresh_channel_senders(&mut self, cx: &mut Context<Self>) {
        let rooms: Vec<Id> = self.channel_rows.keys().cloned().collect();
        let mut changed_rooms = Vec::new();
        let mut avatars = Vec::new();
        for room_id in rooms {
            let Some(rows) = self.channel_rows.get(&room_id) else {
                continue;
            };
            let mut rows = rows.clone();
            let mut changed = false;
            for m in rows.iter_mut().filter(|m| {
                m.sender.starts_with("user:")
                    && (m.sender_name.as_deref().is_none_or(str::is_empty)
                        || m.sender_avatar.as_deref().is_none_or(str::is_empty))
            }) {
                if self.fill_channel_sender(m) {
                    changed = true;
                    if let Some(url) = m.sender_avatar.clone().filter(|u| !u.is_empty()) {
                        avatars.push(url);
                    }
                }
            }
            if changed {
                self.channel_rows.insert(room_id.clone(), rows);
                changed_rooms.push(room_id);
            }
        }
        for url in avatars {
            self.want_avatar(url);
        }
        for room_id in changed_rooms {
            cx.emit(AppEvent::TranscriptChanged { chat_id: room_id });
        }
    }

    /// Members come only with `GET /servers/{id}`: ask once per server per session for
    /// any server the list brought without them, so channel bubbles and the list's
    /// previews can name people from the first paint.
    fn want_server_rosters(&mut self) {
        let wanted: Vec<Id> = self
            .servers
            .iter()
            .filter(|s| s.members.is_empty())
            .map(|s| s.id.clone())
            .filter(|id| !self.server_rosters_requested.contains(id))
            .collect();
        for server_id in wanted {
            self.server_rosters_requested.insert(server_id.clone());
            self.command(ClientCommand::ServerFetch { server_id });
        }
    }

    /// Composer send in a channel: optimistic row (id = ULID) that the echo replaces.
    fn send_to_channel(
        &mut self,
        room_id: Id,
        text: String,
        extras: usize,
        cx: &mut Context<Self>,
    ) {
        if extras > 0 {
            self.push_notice("Attachments are not supported in channels yet");
        }
        let text = text.trim().to_string();
        if text.is_empty() {
            cx.notify();
            return;
        }
        if !self.channel_can_post() {
            return;
        }
        let local_id = new_id();
        {
            let ui = self.chat_ui.entry(room_id.clone()).or_default();
            if !ui.composer_dropped {
                ui.composer_dropped = true;
                ui.drop_started_at = Some(now_ms());
            }
            ui.reply_to = None;
            ui.editing = None;
        }
        let me = self.me_member();
        let handle = self.me_handle();
        let avatar = self.account.as_ref().map(|a| a.avatar_url.clone());
        self.channel_rows
            .entry(room_id.clone())
            .or_default()
            .push(Message {
                id: local_id.clone(),
                chat_id: room_id.clone(),
                role: Role::User,
                sender: me,
                status: MessageStatus::Done,
                turn: None,
                decision: None,
                subagent_id: None,
                subagent_name: None,
                handoff_messages: None,
                notice: None,
                worker: false,
                job: None,
                hidden: false,
                agent_reply_to: None,
                reactions: Default::default(),
                in_reply_to: None,
                routine_name: None,
                sender_name: None,
                sender_icon: None,
                sender_handle: Some(handle),
                sender_avatar: avatar,
                sender_avatar_color: None,
                mentions: Vec::new(),
                reply_to: None,
                forwarded_from: None,
                reply_ref: None,
                asked_by_handle: None,
                owner_handle: None,
                usage: None,
                return_contract: None,
                waiting_user: false,
                waiting_approval: false,
                model: None,
                effort: None,
                created_at: now_ms(),
                completed_at: Some(now_ms()),
                call_id: None,
                activity: None,
                pending: true,
                parts: vec![Part::Text {
                    id: new_id(),
                    text: text.clone(),
                }],
                hidden_parts: Vec::new(),
                greeting: false,
                preview: None,
            });
        self.command(ClientCommand::ChannelSend {
            room_id: room_id.clone(),
            local_id,
            text,
        });
        cx.emit(AppEvent::TranscriptChanged { chat_id: room_id });
        cx.notify();
    }

    /// `GET /servers` landed: keep members already fetched, drop servers that left.
    fn on_servers(&mut self, list: Vec<ServerInfo>, cx: &mut Context<Self>) {
        self.servers_available = true;
        let mut merged = Vec::with_capacity(list.len());
        for mut s in list {
            if let Some(old) = self.servers.iter().find(|o| o.id == s.id) {
                if s.members.is_empty() {
                    s.members = old.members.clone();
                }
            }
            merged.push(s);
        }
        let gone: Vec<Id> = self
            .servers
            .iter()
            .filter(|o| !merged.iter().any(|s| s.id == o.id))
            .map(|o| o.id.clone())
            .collect();
        for url in merged.iter().filter_map(|s| s.image_url.clone()) {
            self.want_avatar(url);
        }
        self.servers = merged;
        for id in gone {
            self.drop_server(&id, cx);
        }
        self.want_member_avatars();
        self.want_server_rosters();
        self.refresh_channel_senders(cx);
        self.resolve_pending_channel(cx);
        self.apply_open_server_hook(cx);
        cx.notify();
    }

    /// Rig: `ARBOS_OPEN_SERVER=<name or id>[:<#channel>][:panel|:channel]` enters a server once
    /// the registry lists it (the row click, landing on `#general`), then the named channel;
    /// `panel` opens the server's page (the server-header click), `channel` the channel's (the
    /// pill click).
    pub fn apply_open_server_hook(&mut self, cx: &mut Context<Self>) {
        // A first login still running would select its chat over the server; after it ends.
        if self.open_server_hook_done || self.onboarding.is_some() {
            return;
        }
        let Ok(spec) = std::env::var("ARBOS_OPEN_SERVER") else {
            return;
        };
        let mut parts = spec.split(':');
        let key = parts.next().unwrap_or_default();
        let Some(server) = self
            .servers
            .iter()
            .find(|s| s.id == key || s.name.eq_ignore_ascii_case(key))
            .cloned()
        else {
            return;
        };
        self.open_server_hook_done = true;
        self.enter_server(server.id.clone(), cx);
        for part in parts {
            if part == "panel" || part == "panel-members" || part == "panel-settings" {
                self.open_profile(
                    ProfilePage::Server {
                        server_id: server.id.clone(),
                    },
                    cx,
                );
                match part {
                    "panel-members" => self.set_profile_tab(1, cx),
                    "panel-settings" => self.set_profile_tab(2, cx),
                    _ => {}
                }
            } else if part == "add-members" {
                self.open_add_server_members(server.id.clone(), cx);
            } else if part == "channel" || part == "channel-settings" {
                if let Some(c) = self.selected_channel.clone() {
                    self.open_profile(
                        ProfilePage::Channel {
                            server_id: c.server_id,
                            channel_id: c.channel_id,
                        },
                        cx,
                    );
                    if part == "channel-settings" {
                        self.set_profile_tab(1, cx);
                    }
                }
            } else if part == "menu" || part == "menu-delete" {
                self.rig_channel_menu = Some(part == "menu-delete");
            } else if part == "archive" {
                if let Some(c) = self.selected_channel.clone() {
                    self.archive_channel(c.server_id, c.channel_id, true, cx);
                }
            } else if let Some(channel) = server
                .channels
                .iter()
                .find(|c| c.name == part.trim_start_matches('#'))
            {
                self.select_channel(server.id.clone(), channel.id.clone(), cx);
            }
        }
    }

    /// One server with members (the detail route or a write's answer).
    fn on_server_detail(&mut self, server: ServerInfo, cx: &mut Context<Self>) {
        self.server_busy = false;
        self.server_error = None;
        if self.server_sheet.is_some() {
            // A channel was created: the sheet is done.
            self.server_sheet = None;
        }
        if let Some(url) = server.image_url.clone() {
            self.want_avatar(url);
        }
        match self.servers.iter_mut().find(|s| s.id == server.id) {
            Some(slot) => *slot = server,
            None => self.servers.push(server),
        }
        self.want_member_avatars();
        self.refresh_channel_senders(cx);
        self.resolve_pending_channel(cx);
        cx.notify();
    }

    /// Created or joined: the sheet closes and the server opens on `#general`.
    fn on_server_opened(&mut self, server: ServerInfo, cx: &mut Context<Self>) {
        self.server_busy = false;
        self.server_error = None;
        self.server_sheet = None;
        let id = server.id.clone();
        match self.servers.iter_mut().find(|s| s.id == id) {
            Some(slot) => *slot = server,
            None => self.servers.push(server),
        }
        self.want_member_avatars();
        self.want_server_rosters();
        self.enter_server(id, cx);
    }

    fn want_member_avatars(&mut self) {
        let urls: Vec<String> = self
            .servers
            .iter()
            .flat_map(|s| s.members.iter())
            .map(|m| m.avatar_url.clone())
            .filter(|u| !u.is_empty())
            .collect();
        for url in urls {
            self.want_avatar(url);
        }
    }

    /// The homeserver's room list changed: previews, unread, and rooms of fresh channels.
    fn on_matrix_rooms(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        self.matrix_snapshot = snapshot;
        self.resolve_pending_channel(cx);
        // A channel the registry does not know yet (joined elsewhere): refetch the list.
        let unknown = self
            .matrix_snapshot
            .spaces
            .iter()
            .any(|s| !s.server_id.is_empty() && self.server(&s.server_id).is_none());
        if unknown && self.servers_available {
            self.command(ClientCommand::ServersRefresh);
        }
        cx.notify();
    }

    /// Rows of a channel (the first page, a page of history, or the whole list after a new row).
    /// Optimistic rows not echoed yet stay at the end.
    fn on_matrix_timeline(
        &mut self,
        room_id: String,
        rows: Vec<MessageRow>,
        cx: &mut Context<Self>,
    ) {
        let mut decoded: Vec<Message> = rows
            .iter()
            .filter_map(|r| self.channel_message(&room_id, r))
            .collect();
        let echoed: HashSet<Id> = decoded.iter().map(|m| m.id.clone()).collect();
        if let Some(old) = self.channel_rows.get(&room_id) {
            for m in old.iter().filter(|m| m.pending && !echoed.contains(&m.id)) {
                decoded.push(m.clone());
            }
        }
        let urls: Vec<String> = decoded
            .iter()
            .filter_map(|m| m.sender_avatar.clone())
            .filter(|u| !u.is_empty())
            .collect();
        for url in urls {
            self.want_avatar(url);
        }
        let count = decoded.len();
        self.channel_rows.insert(room_id.clone(), decoded);
        self.index_chat(&room_id);
        // A full first page means there is more above: fetch one more page so the room opens
        // with a comfortable scrollback (one extra page per session; the rest on demand later).
        if count >= CHANNEL_PAGE && self.channel_paged.insert(room_id.clone()) {
            self.command(ClientCommand::ChannelLoadOlder {
                room_id: room_id.clone(),
            });
        }
        let active = self
            .selected_channel
            .as_ref()
            .and_then(|c| c.room_id.as_deref())
            == Some(room_id.as_str());
        if active && self.window_active {
            self.command(ClientCommand::ChannelMarkRead {
                room_id: room_id.clone(),
            });
        }
        cx.emit(AppEvent::TranscriptChanged { chat_id: room_id });
        cx.notify();
    }

    fn on_channel_send_failed(
        &mut self,
        room_id: String,
        local_id: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(rows) = self.channel_rows.get_mut(&room_id) {
            rows.retain(|m| m.id != local_id);
        }
        cx.emit(AppEvent::TranscriptChanged { chat_id: room_id });
        cx.notify();
    }

    // ----- voice (F11) -----------------------------------------------------

    /// Voice mode exists for this user: the relay flag is on. The agent pane shows its switch
    /// and voice row, and the transcript its speaker rows, only then.
    pub fn voice_mode_available(&self) -> bool {
        self.voice_mode_feature
    }

    pub fn set_voice_mode(&mut self, on: bool, cx: &mut Context<Self>) {
        if on && !self.voice_mode_feature {
            return;
        }
        if self.voice_mode != on {
            self.voice_mode = on;
            if let Some(chat_id) = self.active_chat_id() {
                cx.emit(AppEvent::TranscriptChanged { chat_id });
            }
            cx.notify();
        }
    }

    /// Speech-to-text of a 16 kHz mono WAV through the relay proxy; the answer
    /// arrives as [`AppEvent::VoiceTranscript`] with the same `request_id`.
    pub fn transcribe(&mut self, request_id: u64, wav: Arc<Vec<u8>>) {
        let project_id = self.selected_project.clone().unwrap_or_default();
        self.command(ClientCommand::Transcribe {
            project_id,
            request_id,
            wav,
        });
    }

    /// Text-to-speech through the relay proxy; answered by [`AppEvent::VoiceSpeech`].
    pub fn speak(&mut self, request_id: u64, text: String) {
        let project_id = self.selected_project.clone().unwrap_or_default();
        self.command(ClientCommand::Speak {
            project_id,
            request_id,
            text,
        });
    }

    /// The reply's speech is cached: show the speaker icon under it.
    pub fn mark_spoken(&mut self, message_id: Id, cx: &mut Context<Self>) {
        if self.spoken.insert(message_id.clone()) {
            if let Some(chat_id) = self.chat_of_message(&message_id) {
                cx.emit(AppEvent::TranscriptChanged { chat_id });
            }
            cx.notify();
        }
    }

    pub fn replay_speech(&mut self, message_id: Id, cx: &mut Context<Self>) {
        cx.emit(AppEvent::ReplaySpeech { message_id });
    }

    /// Speech for this reply did not arrive, decode or play.
    pub fn mark_speech_failed(&mut self, message_id: Id, reason: String, cx: &mut Context<Self>) {
        let chat_id = self.chat_of_message(&message_id);
        self.speech_failed.insert(message_id, reason);
        if let Some(chat_id) = chat_id {
            cx.emit(AppEvent::TranscriptChanged { chat_id });
        }
        cx.notify();
    }

    /// The reply plays now: forget the earlier failure.
    pub fn clear_speech_failed(&mut self, message_id: &str, cx: &mut Context<Self>) {
        if self.speech_failed.remove(message_id).is_some() {
            if let Some(chat_id) = self.chat_of_message(message_id) {
                cx.emit(AppEvent::TranscriptChanged { chat_id });
            }
            cx.notify();
        }
    }

    pub fn chat_of_message(&self, message_id: &str) -> Option<Id> {
        self.messages
            .iter()
            .find(|(_, list)| list.iter().any(|m| m.id == message_id))
            .map(|(chat_id, _)| chat_id.clone())
    }

    /// Answer a typed widget card: the rendered text becomes the user's bubble at once
    /// (optimistic), the card flips to answered, and the structured value goes to the box.
    pub fn answer_question(
        &mut self,
        chat_id: Id,
        question_id: String,
        value: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        let widget = self.messages_of(&chat_id).iter().find_map(|m| {
            m.parts.iter().find_map(|p| match p {
                Part::Question {
                    question_id: qid,
                    widget,
                    ..
                } if *qid == question_id => Some(widget.clone()),
                _ => None,
            })
        });
        let text = match widget.flatten() {
            Some(w) => w.answer_text(&value),
            None => format!(
                "Answer: {}",
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string())
            ),
        };
        let message_id = new_id();
        {
            let ui = self.chat_ui.entry(chat_id.clone()).or_default();
            if !ui.composer_dropped {
                ui.composer_dropped = true;
                ui.drop_started_at = Some(now_ms());
            }
        }
        self.mark_question_answered(&chat_id, &question_id, Some(value.clone()));
        self.messages
            .entry(chat_id.clone())
            .or_default()
            .push(Message {
                id: message_id.clone(),
                chat_id: chat_id.clone(),
                role: Role::User,
                sender: "user".into(),
                status: MessageStatus::Queued,
                turn: None,
                decision: None,
                subagent_id: None,
                subagent_name: None,
                handoff_messages: None,
                notice: None,
                worker: false,
                job: None,
                hidden: false,
                agent_reply_to: None,
                reactions: Default::default(),
                in_reply_to: None,
                usage: None,
                return_contract: None,
                waiting_user: false,
                waiting_approval: false,
                model: None,
                effort: None,
                routine_name: None,
                sender_name: None,
                sender_icon: None,
                sender_handle: None,
                sender_avatar: None,
                sender_avatar_color: None,
                mentions: Vec::new(),
                reply_to: None,
                forwarded_from: None,
                reply_ref: None,
                asked_by_handle: None,
                owner_handle: None,
                created_at: now_ms(),
                completed_at: None,
                call_id: None,
                activity: None,
                pending: true,
                parts: vec![Part::Text {
                    id: new_id(),
                    text: text.clone(),
                }],
                hidden_parts: Vec::new(),
                greeting: false,
                preview: None,
            });
        self.command(ClientCommand::Answer {
            chat_id: chat_id.clone(),
            message_id,
            text,
            question_id: Some(question_id),
            value: Some(value),
        });
        cx.emit(AppEvent::TranscriptChanged { chat_id });
        cx.notify();
    }

    fn mark_question_answered(
        &mut self,
        chat_id: &str,
        question_id: &str,
        value: Option<serde_json::Value>,
    ) {
        let Some(list) = self.messages.get_mut(chat_id) else {
            return;
        };
        for m in list.iter_mut() {
            for p in m.parts.iter_mut() {
                if let Part::Question {
                    question_id: qid,
                    answer,
                    state,
                    answered_at,
                    ..
                } = p
                {
                    if qid == question_id {
                        *answer = value.clone();
                        *state = QuestionState::Answered;
                        *answered_at = Some(now_ms());
                        m.waiting_user = false;
                    }
                }
            }
        }
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(chat_id) = self.active_chat_id() {
            if let Some(job_id) = crate::jobtabs::job_id(&chat_id).map(str::to_owned) {
                // A job tab's Stop halts that job only (`stop { jobId }`).
                if let Some(parent) = self.job_parent_chat(&job_id) {
                    self.stop_job(parent, &job_id, cx);
                }
                return;
            }
            self.stop_all_jobs(&chat_id);
            self.command(ClientCommand::Stop { chat_id });
        }
        cx.notify();
    }

    // ----- tasks chip and drawer (duplex §2.4, D7) ------------------------------

    /// The open background jobs of `chat_id`, newest step first as the relay
    /// lists them. Empty when the relay predates `activity.jobs[]`, when no job
    /// runs, or when every job row of the chat has closed (the box finished them;
    /// the next poll confirms).
    pub fn open_jobs(&self, chat_id: &str) -> Vec<Job> {
        let Some(project) = self.project_of_chat(chat_id) else {
            return Vec::new();
        };
        if project.is_people() {
            // A group / DM: the jobs any agent in it started from this chat (each agent's
            // own listing names the chat).
            return self
                .jobs
                .values()
                .flatten()
                .filter(|j| j.chat_id.as_deref() == Some(chat_id))
                .cloned()
                .collect();
        }
        let Some(jobs) = self.jobs.get(&project.id) else {
            return Vec::new();
        };
        jobs.iter()
            .filter(|j| j.chat_id.as_deref().is_none_or(|c| c == chat_id))
            .cloned()
            .collect()
    }

    /// The agent projects whose listings the open chat needs: its own agent, or every
    /// agent in a group / DM.
    fn jobs_projects_for(&self, chat_id: &str) -> Vec<Id> {
        let Some(project) = self.project_of_chat(chat_id) else {
            return Vec::new();
        };
        if project.is_people() {
            project
                .agents
                .iter()
                .map(|a| a.project_id.clone())
                .filter(|pid| self.projects.iter().any(|p| &p.id == pid && !p.is_people()))
                .collect()
        } else {
            vec![project.id.clone()]
        }
    }

    /// Something to poll for: a job row of the open chat still streams, the
    /// relay listed a job last time, or the drawer is up.
    fn jobs_poll_wanted(&self) -> Vec<Id> {
        let Some(chat_id) = self.active_chat_id() else {
            return Vec::new();
        };
        let projects = self.jobs_projects_for(&chat_id);
        if projects.is_empty() {
            return Vec::new();
        }
        let live = Message::chat_has_live_job(self.messages_of(&chat_id), now_ms());
        let listed = projects
            .iter()
            .any(|pid| self.jobs.get(pid).is_some_and(|j| !j.is_empty()));
        if live || listed || self.tasks_open.is_some() {
            projects
        } else {
            Vec::new()
        }
    }

    fn jobs_tick(&mut self) {
        for project_id in self.jobs_poll_wanted() {
            self.command(ClientCommand::JobsRefresh { project_id });
        }
        self.rollout_tick();
    }

    // ----- job tabs (the sub-agent's rollout as a chat tab, `jobtabs`) --------------

    /// The project a job belongs to: the one whose listing or tabs hold it.
    pub fn job_project(&self, job_id: &str) -> Option<Id> {
        if let Some((pid, _)) = self
            .jobs
            .iter()
            .find(|(_, jobs)| jobs.iter().any(|j| j.id == job_id))
        {
            return Some(pid.clone());
        }
        let chat = crate::jobtabs::chat_id(job_id);
        self.job_tabs
            .iter()
            .find(|(_, tabs)| tabs.iter().any(|c| c.id == chat))
            .map(|(pid, _)| pid.clone())
    }

    /// The chip's row for a job, wherever it is listed.
    fn job_row(&self, job_id: &str) -> Option<&Job> {
        self.jobs.values().flatten().find(|j| j.id == job_id)
    }

    /// The chat the job was asked in (its parent chat): the document's word, else the chip's.
    fn job_parent_chat(&self, job_id: &str) -> Option<Id> {
        self.rollouts
            .get(job_id)
            .and_then(|r| r.chat_id.clone())
            .or_else(|| self.job_row(job_id).and_then(|j| j.chat_id.clone()))
    }

    /// The listing changed: every job the agent runs has a tab (unless the user closed it);
    /// tabs of finished jobs stay until closed.
    fn sync_job_tabs(&mut self, project_id: &Id) {
        let now = now_ms();
        let listed: Vec<Job> = self.jobs.get(project_id).cloned().unwrap_or_default();
        let closed = self.closed_job_tabs.clone();
        let tabs = self.job_tabs.entry(project_id.clone()).or_default();
        for job in listed {
            if closed.contains(&job.id) {
                continue;
            }
            let id = crate::jobtabs::chat_id(&job.id);
            match tabs.iter_mut().find(|c| c.id == id) {
                Some(tab) => {
                    if !job.title.trim().is_empty() {
                        tab.title = job.title.clone();
                    }
                }
                None => tabs.push(crate::jobtabs::tab(project_id, &job.id, &job.title, now)),
            }
        }
    }

    /// Open (and select) the tab of `job_id`: from a group chat this jumps to the agent's
    /// own space first. A tab the user closed comes back.
    pub fn open_job_tab(&mut self, job_id: &str, cx: &mut Context<Self>) {
        let Some(project_id) = self.job_project(job_id) else {
            return;
        };
        self.closed_job_tabs.remove(job_id);
        let title = self
            .job_row(job_id)
            .map(|j| j.title.clone())
            .or_else(|| self.rollouts.get(job_id).map(|r| r.title.clone()))
            .unwrap_or_default();
        let chat_id = crate::jobtabs::chat_id(job_id);
        let tabs = self.job_tabs.entry(project_id.clone()).or_default();
        if !tabs.iter().any(|c| c.id == chat_id) {
            tabs.push(crate::jobtabs::tab(&project_id, job_id, &title, now_ms()));
        }
        self.tasks_open = None;
        if self.selected_project.as_deref() != Some(project_id.as_str())
            || self.selected_channel.is_some()
        {
            self.select_project(project_id.clone(), cx);
        }
        self.select_tab(chat_id.clone(), cx);
        self.refresh_job_tab(&chat_id);
        cx.notify();
    }

    /// The status line's link: the newest open job of the chat.
    pub fn open_newest_job_tab(&mut self, chat_id: Id, cx: &mut Context<Self>) {
        if let Some(job) = self.open_jobs(&chat_id).into_iter().next() {
            self.open_job_tab(&job.id, cx);
        }
    }

    /// Close a job tab: back to the agent's chat; the job itself goes on.
    fn close_job_tab(&mut self, chat_id: &str, cx: &mut Context<Self>) -> bool {
        let Some(job_id) = crate::jobtabs::job_id(chat_id).map(str::to_owned) else {
            return false;
        };
        let Some(project_id) = self.job_project(&job_id) else {
            return false;
        };
        if let Some(tabs) = self.job_tabs.get_mut(&project_id) {
            tabs.retain(|c| c.id != chat_id);
        }
        self.closed_job_tabs.insert(job_id.clone());
        self.messages.remove(chat_id);
        self.pending_steers.remove(&job_id);
        if self.active_tab.get(&project_id).map(String::as_str) == Some(chat_id) {
            if let Some(primary) = self.primary_chat(&project_id).map(|c| c.id.clone()) {
                self.select_tab(primary, cx);
            }
        }
        cx.notify();
        true
    }

    /// Ask for the tab's exchange: the whole document first, then only the entries past
    /// the newest one held.
    fn refresh_job_tab(&self, chat_id: &str) {
        let Some(job_id) = crate::jobtabs::job_id(chat_id) else {
            return;
        };
        let Some(project_id) = self.job_project(job_id) else {
            return;
        };
        let after = self
            .rollouts
            .get(job_id)
            .filter(|r| !r.entries.is_empty())
            .map(|r| r.next_seq.saturating_sub(1));
        self.command(ClientCommand::RolloutRefresh {
            project_id,
            job_id: job_id.to_string(),
            after,
        });
    }

    /// Each tick: the active job tab polls while its job runs (or until its document arrives).
    fn rollout_tick(&mut self) {
        let Some(chat_id) = self.active_chat_id() else {
            return;
        };
        let Some(job_id) = crate::jobtabs::job_id(&chat_id) else {
            return;
        };
        let wanted = match self.rollouts.get(job_id) {
            Some(r) => !r.unavailable && r.is_running(),
            None => true,
        };
        if wanted {
            self.refresh_job_tab(&chat_id);
        }
    }

    /// A job document arrived: the tab's transcript is rebuilt from it.
    pub(crate) fn apply_rollout(
        &mut self,
        job_id: &str,
        rollout: JobRollout,
        cx: &mut Context<Self>,
    ) {
        match self.rollouts.get_mut(job_id) {
            Some(held) if rollout.unavailable => held.unavailable = true,
            Some(held) => held.merge(rollout),
            None => {
                self.rollouts.insert(job_id.to_string(), rollout);
            }
        }
        let chat_id = crate::jobtabs::chat_id(job_id);
        let Some(doc) = self.rollouts.get(job_id).cloned() else {
            return;
        };
        let pending = self.pending_steers.get(job_id).cloned().unwrap_or_default();
        let messages = crate::jobtabs::messages(&doc, &pending);
        if let Some(project_id) = self.job_project(job_id) {
            if let Some(tab) = self
                .job_tabs
                .get_mut(&project_id)
                .and_then(|tabs| tabs.iter_mut().find(|c| c.id == chat_id))
            {
                if !doc.title.trim().is_empty() {
                    tab.title = doc.title.clone();
                }
                if let Some(last) = messages.iter().map(|m| m.created_at).max() {
                    tab.last_message_at = tab.last_message_at.max(last);
                }
            }
        }
        self.messages.insert(chat_id.clone(), messages);
        cx.emit(AppEvent::TranscriptChanged { chat_id });
        cx.notify();
    }

    /// Typing in a job tab steers the job: the words go to the parent chat as a reply to the
    /// job's ask (the wire the drawer's Steer uses) and show in the tab at once.
    fn steer_from_job_tab(&mut self, chat_id: &Id, text: String, cx: &mut Context<Self>) {
        let Some(job_id) = crate::jobtabs::job_id(chat_id).map(str::to_owned) else {
            return;
        };
        let parent_chat = self.job_parent_chat(&job_id);
        let ask = self
            .rollouts
            .get(&job_id)
            .and_then(|r| r.ask_message_id.clone())
            .or_else(|| self.job_row(&job_id).and_then(|j| j.ask_message_id.clone()));
        let (Some(parent_chat), Some(ask)) = (parent_chat, ask) else {
            self.push_notice(
                "This task cannot be steered from here yet — reply to your request in the chat instead.",
            );
            cx.notify();
            return;
        };
        let message_id = new_id();
        let echo = crate::jobtabs::typed_steer(chat_id, &message_id, &text, &ask);
        self.pending_steers
            .entry(job_id.clone())
            .or_default()
            .push(echo.clone());
        self.messages
            .entry(chat_id.clone())
            .or_default()
            .push(echo.clone());
        // The parent chat carries the real message: a reply under the ask.
        self.messages
            .entry(parent_chat.clone())
            .or_default()
            .push(Message {
                chat_id: parent_chat.clone(),
                status: MessageStatus::Queued,
                ..echo
            });
        self.command(ClientCommand::Send {
            chat_id: parent_chat.clone(),
            message_id,
            text,
            attachments: Vec::new(),
            reply_to: Some(ask),
            reply_ref: None,
            mentions: Vec::new(),
        });
        cx.emit(AppEvent::TranscriptChanged {
            chat_id: chat_id.clone(),
        });
        cx.emit(AppEvent::TranscriptChanged {
            chat_id: parent_chat,
        });
        cx.notify();
    }

    /// The chip was clicked: the drawer opens over the presence row (or closes).
    pub fn toggle_tasks(&mut self, chat_id: Id, cx: &mut Context<Self>) {
        if self.tasks_open.as_deref() == Some(chat_id.as_str()) {
            self.tasks_open = None;
        } else if !self.open_jobs(&chat_id).is_empty() {
            self.tasks_open = Some(chat_id);
        }
        cx.notify();
    }

    pub fn close_tasks(&mut self, cx: &mut Context<Self>) {
        if self.tasks_open.take().is_some() {
            cx.notify();
        }
    }

    /// Steer a job (contract: no command of its own): the composer becomes a
    /// reply to the job's ask and the user types the correction; the agent
    /// binds a correction that quotes the ask to that job.
    pub fn steer_job(&mut self, chat_id: Id, job: &Job, cx: &mut Context<Self>) -> bool {
        let Some(ask) = job.ask_message_id.clone().filter(|a| !a.is_empty()) else {
            return false;
        };
        self.set_reply_to(chat_id, ask, cx);
        self.tasks_open = None;
        cx.notify();
        true
    }

    /// `stop { jobId }`: that job only. The row leaves the drawer at once; the
    /// next poll is the box's word.
    pub fn stop_job(&mut self, chat_id: Id, job_id: &str, cx: &mut Context<Self>) {
        let message_id = self
            .messages_of(&chat_id)
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .map(|m| m.id.clone())
            .unwrap_or_default();
        if let Some(project_id) = self.project_of_chat(&chat_id).map(|p| p.id.clone()) {
            if let Some(jobs) = self.jobs.get_mut(&project_id) {
                jobs.retain(|j| j.id != job_id);
                if jobs.is_empty() {
                    self.jobs.remove(&project_id);
                    self.tasks_open = None;
                }
            }
        }
        self.command(ClientCommand::StopJob {
            chat_id,
            job_id: job_id.to_string(),
            message_id,
        });
        cx.notify();
    }

    /// The drawer's Stop all = the Stop button: `stop {}` halts the turn and
    /// every job of the chat (D7). Forgets the chat's jobs at once.
    fn stop_all_jobs(&mut self, chat_id: &str) {
        if let Some(project_id) = self.project_of_chat(chat_id).map(|p| p.id.clone()) {
            if let Some(jobs) = self.jobs.get_mut(&project_id) {
                jobs.retain(|j| j.chat_id.as_deref().is_some_and(|c| c != chat_id));
                if jobs.is_empty() {
                    self.jobs.remove(&project_id);
                }
            }
        }
        if self.tasks_open.as_deref() == Some(chat_id) {
            self.tasks_open = None;
        }
    }

    pub fn stop_all(&mut self, chat_id: Id, cx: &mut Context<Self>) {
        self.stop_all_jobs(&chat_id);
        self.command(ClientCommand::Stop { chat_id });
        cx.notify();
    }

    /// Add or remove my emoji reaction on a message (several emojis may sit on
    /// one message; the same emoji again clears it).
    pub fn toggle_reaction(
        &mut self,
        chat_id: Id,
        message_id: Id,
        emoji: String,
        cx: &mut Context<Self>,
    ) {
        if emoji.is_empty() || self.me_id().is_empty() {
            return;
        }
        self.command(ClientCommand::React {
            chat_id,
            message_id,
            emoji,
        });
        cx.notify();
    }

    /// Delete a message for everyone through the shared chat doc (the
    /// message menu's Delete, #1370; the iPhone's `deleteMessage`).
    pub fn delete_message(&mut self, chat_id: Id, message_id: Id, cx: &mut Context<Self>) {
        if message_id.is_empty() {
            return;
        }
        self.command(ClientCommand::DeleteMessage {
            chat_id,
            message_id,
        });
        cx.notify();
    }

    /// Save an in-place edit of an own message's text (no new send). Peers see
    /// the updated body via room sync, same as the iPhone's `editMessage`.
    pub fn edit_message(
        &mut self,
        chat_id: Id,
        message_id: Id,
        text: String,
        cx: &mut Context<Self>,
    ) {
        let trimmed = text.trim().to_string();
        if message_id.is_empty() || trimmed.is_empty() {
            return;
        }
        self.command(ClientCommand::EditMessage {
            chat_id: chat_id.clone(),
            message_id,
            text: trimmed,
        });
        if let Some(ui) = self.chat_ui.get_mut(&chat_id) {
            ui.editing = None;
        }
        cx.notify();
    }

    /// Bubble ⋯ / the card's × → Remove preview: the relay drops the message's link preview
    /// for everyone (`preview: "off"`); the card goes at once here, the row follows.
    pub fn remove_link_preview(&mut self, chat_id: Id, message_id: Id, cx: &mut Context<Self>) {
        if message_id.is_empty() {
            return;
        }
        if let Some(m) = self
            .messages
            .get_mut(&chat_id)
            .and_then(|list| list.iter_mut().find(|m| m.id == message_id))
        {
            m.preview = Some("off".into());
        }
        self.command(ClientCommand::RemoveLinkPreview {
            chat_id,
            message_id,
        });
        cx.notify();
    }

    /// A link in a bubble or a preview card was clicked: one seam for every chat link, so a
    /// confirm step (#1522) slots in here for all of them at once. Today: the default opener.
    pub fn open_link(&mut self, url: &str, cx: &mut Context<Self>) {
        if url.trim().is_empty() {
            return;
        }
        cx.open_url(url);
    }

    /// Settings → Chat → Link previews: cards on or off on this Mac.
    pub fn set_link_previews(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.link_previews == enabled {
            return;
        }
        self.link_previews = enabled;
        crate::ui::link_card::set_enabled(enabled);
        save_link_previews(enabled);
        cx.notify();
    }

    /// Retry on one of my bubbles the relay refused (`MessageStatus::Error`,
    /// see `MessageSendFailed`): the failed row goes, and its text + reply-to go
    /// out again as a new message through the normal send path (attachments
    /// were local files picked at send time and are not re-read).
    pub fn resend_failed(&mut self, chat_id: Id, message_id: Id, cx: &mut Context<Self>) {
        let Some(list) = self.messages.get_mut(&chat_id) else {
            return;
        };
        let Some(ix) = list
            .iter()
            .position(|m| m.id == message_id && m.status == MessageStatus::Error)
        else {
            return;
        };
        let failed = list.remove(ix);
        let text = failed.plain_text();
        if text.trim().is_empty() {
            cx.emit(AppEvent::TranscriptChanged { chat_id });
            cx.notify();
            return;
        }
        self.send(text, Vec::new(), failed.reply_to.clone(), Vec::new(), cx);
    }

    pub fn retry(&mut self, chat_id: Id, message_id: Id, cx: &mut Context<Self>) {
        self.command(ClientCommand::Retry {
            chat_id,
            message_id,
        });
        cx.notify();
    }

    /// D76/D83: one of the four answers on a secret-request card. The relay row is the
    /// authority; the doc command then flips the card for agentd.
    pub fn answer_approval(
        &mut self,
        chat_id: Id,
        approval_id: String,
        choice: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(request_id) = self.secret_request_of(&chat_id, &approval_id) {
            self.secret_decide(request_id, choice.to_string(), cx);
        }
        if choice == "deny" {
            self.command(ClientCommand::Reject {
                chat_id,
                approval_id,
                reason: Some("denied".into()),
            });
        } else {
            // #1544: "grant:<path class>" is a destructive-shell card's standing option.
            let grant = choice.strip_prefix("grant:").map(str::to_owned);
            self.command(ClientCommand::Approve {
                chat_id,
                approval_id,
                grant,
            });
        }
        cx.notify();
    }

    /// Secret hand-off request card → Save: the value leaves this device once, to the vault.
    #[allow(clippy::too_many_arguments)] // one call site, the card; the fields mirror the command
    pub fn secret_handoff_save(
        &mut self,
        chat_id: Id,
        approval_id: String,
        project_id: Id,
        name: String,
        value: String,
        purpose: String,
        cx: &mut Context<Self>,
    ) {
        self.handoff
            .insert(approval_id.clone(), HandoffState::Saving);
        self.command(ClientCommand::SecretHandoffSave {
            chat_id,
            approval_id,
            project_id,
            name,
            value,
            purpose,
        });
        cx.notify();
    }

    /// S18: approve an approval card; the box dispatches and flips the card's state.
    pub fn approve(&mut self, chat_id: Id, approval_id: String, cx: &mut Context<Self>) {
        self.forward_vault_decision(&chat_id, &approval_id, true, cx);
        self.command(ClientCommand::Approve {
            chat_id,
            approval_id,
            grant: None,
        });
        cx.notify();
    }

    /// S18: reject an approval card; nothing is dispatched.
    pub fn reject(
        &mut self,
        chat_id: Id,
        approval_id: String,
        reason: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.forward_vault_decision(&chat_id, &approval_id, false, cx);
        self.command(ClientCommand::Reject {
            chat_id,
            approval_id,
            reason,
        });
        cx.notify();
    }

    /// F8: a vault card carries the relay request id in its payload; the relay row is the
    /// authority (`POST /vault/requests/{id}/decide` needs a device token), so the card's
    /// decision goes there too.
    fn forward_vault_decision(
        &mut self,
        chat_id: &Id,
        approval_id: &str,
        approve: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(request_id) = self.secret_request_of(chat_id, approval_id) {
            self.secret_decide(
                request_id,
                if approve { "always" } else { "deny" }.into(),
                cx,
            );
        }
    }

    /// The relay request id a secret card carries (`secretRequestId`, or F8's `vaultRequestId`).
    fn secret_request_of(&self, chat_id: &Id, approval_id: &str) -> Option<String> {
        self.messages
            .get(chat_id)
            .into_iter()
            .flatten()
            .flat_map(|m| m.parts.iter())
            .find_map(|p| match p {
                Part::Approval {
                    approval_id: id,
                    payload,
                    ..
                } if id == approval_id => payload
                    .get("secretRequestId")
                    .or_else(|| payload.get("vaultRequestId"))
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
                _ => None,
            })
    }

    /// The agent's model (profile page picker) — optimistic, then `SetModel`
    /// with its current effort.
    pub fn set_project_model(&mut self, project_id: Id, model: String, cx: &mut Context<Self>) {
        let mut effort = Effort::High;
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == project_id) {
            p.model = model.clone();
            effort = p.effort;
        }
        self.command(ClientCommand::SetModel {
            project_id,
            model,
            effort,
        });
        cx.notify();
    }

    /// The agent's call voice (`None` = the current default); saved on click.
    pub fn set_voice(&mut self, project_id: Id, voice: Option<String>, cx: &mut Context<Self>) {
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == project_id) {
            p.voice = voice.clone();
        }
        self.command(ClientCommand::SetVoice { project_id, voice });
        cx.notify();
    }

    /// Call screen, "Let <agent> see" (#1353): may this agent look through my
    /// camera on calls. Persists to the agent's `vision_enabled`; the voice
    /// worker reads it on the next sample.
    pub fn set_vision(&mut self, project_id: Id, enabled: bool, cx: &mut Context<Self>) {
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == project_id) {
            p.vision_enabled = enabled;
        }
        if !enabled {
            if let Some(c) = self.my_call.as_mut() {
                c.agent_seeing = false;
            }
        }
        self.command(ClientCommand::SetVision {
            project_id,
            enabled,
        });
        cx.notify();
    }

    /// Ask the relay for the voice list (the agent pane's Voice row and the
    /// call-voice picker need it; the list is independent of voice mode).
    pub fn fetch_voices(&mut self) {
        self.command(ClientCommand::FetchVoices);
    }

    /// Play a voice's sample (cached in the config dir after the first fetch);
    /// playing the one already playing stops it.
    pub fn toggle_voice_sample(&mut self, url: String, cx: &mut Context<Self>) {
        if self.sample_playing.as_deref() == Some(url.as_str())
            && self.sample_player.as_ref().is_some_and(|p| p.is_playing())
        {
            self.stop_voice_sample(cx);
            return;
        }
        self.sample_player = None;
        match self.sample_paths.get(&url).cloned() {
            Some(path) if path.is_file() => {
                self.sample_player = Some(SamplePlayer::play(path));
                self.sample_playing = Some(url);
                self.sample_wanted = None;
            }
            _ => {
                self.sample_wanted = Some(url.clone());
                self.sample_playing = Some(url.clone());
                self.command(ClientCommand::FetchVoiceSample { url });
            }
        }
        cx.notify();
    }

    pub fn stop_voice_sample(&mut self, cx: &mut Context<Self>) {
        self.sample_player = None;
        self.sample_playing = None;
        self.sample_wanted = None;
        cx.notify();
    }

    /// True while `url`'s sample is playing (or being fetched to play).
    pub fn voice_sample_playing(&self, url: &str) -> bool {
        self.sample_playing.as_deref() == Some(url)
            && (self.sample_wanted.is_some()
                || self.sample_player.as_ref().is_some_and(|p| p.is_playing()))
    }

    /// The agent's reasoning effort (profile page picker) — optimistic, then
    /// `SetModel` with its current model.
    pub fn set_project_effort(&mut self, project_id: Id, effort: Effort, cx: &mut Context<Self>) {
        let mut model = String::new();
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == project_id) {
            p.effort = effort;
            model = p.model.clone();
        }
        self.command(ClientCommand::SetModel {
            project_id,
            model,
            effort,
        });
        cx.notify();
    }

    /// ⌘B / the sidebar collapse control (next to "+") / the header expand
    /// button when the rail is hidden. In the narrow band the sidebar cannot
    /// dock, so it floats as an overlay instead; otherwise it collapses to the
    /// icon rail or returns, and the choice is saved
    /// ([feedback mac-sidebar-rail-2026-10-26]).
    pub fn toggle_rail(&mut self, narrow: bool, cx: &mut Context<Self>) {
        if narrow {
            self.sidebar_overlay = !self.sidebar_overlay;
        } else {
            self.rail_collapsed = !self.rail_collapsed;
            self.sidebar_overlay = false;
            crate::ui::theme::SidebarCollapsed::save(self.rail_collapsed);
        }
        cx.notify();
    }

    /// A drag of the column's right edge begins (expanded or rail): a fresh
    /// gesture, so a width restored from the rail by an earlier drag no
    /// longer holds the handle ([feedback sidebar-narrow-wrap-2026-10-27]).
    pub fn begin_sidebar_edge_drag(&mut self) {
        self.sidebar_edge_drag = crate::ui::sidebar_fit::EdgeDrag::begin(
            (!self.rail_collapsed).then_some(self.sidebar_width),
        );
    }

    /// Each move of that drag: the pointer's x is the width asked for. Past
    /// the snap point the column collapses to the rail (the saved width is
    /// kept); from the rail, crossing the snap width restores that width,
    /// and the handle follows the pointer again once it passes it
    /// (`ui::sidebar_fit::EdgeDrag`). The rail flip animates like the toggle;
    /// short of the snap width a drag from the rail draws the morph at the
    /// pointer, so every move that changes it is a frame.
    pub fn drag_sidebar_edge(&mut self, x: f32, cx: &mut Context<Self>) {
        use crate::ui::sidebar_fit::EdgeTarget;
        let tracked = self.sidebar_edge_drag.track();
        match self
            .sidebar_edge_drag
            .target(x, self.rail_collapsed, self.sidebar_width)
        {
            EdgeTarget::Rail { keep } => {
                if !self.rail_collapsed {
                    self.rail_collapsed = true;
                    self.sidebar_overlay = false;
                    crate::ui::theme::SidebarCollapsed::save(true);
                    // The width to come back to is the one the drag began
                    // from, not the minimum the handle passed on the way.
                    self.set_sidebar_width(keep, cx);
                    cx.notify();
                }
            }
            EdgeTarget::Expanded(width) => {
                if self.rail_collapsed {
                    self.rail_collapsed = false;
                    crate::ui::theme::SidebarCollapsed::save(false);
                    cx.notify();
                }
                self.set_sidebar_width(width, cx);
            }
            EdgeTarget::Hold => {}
        }
        if self.sidebar_edge_drag.track() != tracked {
            cx.notify();
        }
    }

    /// Drag on the chat-list column's right edge: set width and persist it.
    pub fn set_sidebar_width(&mut self, width: f32, cx: &mut Context<Self>) {
        let width = crate::ui::theme::SidebarWidth::clamp(width);
        if (self.sidebar_width - width).abs() < 0.5 {
            return;
        }
        self.sidebar_width = width;
        crate::ui::theme::SidebarWidth::save(width);
        cx.notify();
    }

    /// Drag on the call strip's handle: the strip's height, already clamped by the caller
    /// (`strip_layout::height_after_drag`). The sidebar is not involved: the strip lives in the
    /// chat column only.
    pub fn set_call_strip_height(&mut self, height: f32, cx: &mut Context<Self>) {
        if (self.call_strip_height - height).abs() < 0.5 {
            return;
        }
        self.call_strip_height = height;
        cx.notify();
    }

    pub fn close_sidebar_overlay(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_overlay {
            self.sidebar_overlay = false;
            cx.notify();
        }
    }

    pub fn machine_start(&mut self, cx: &mut Context<Self>) {
        self.command(ClientCommand::MachineStart);
        cx.notify();
    }

    pub fn machine_stop(&mut self, cx: &mut Context<Self>) {
        self.command(ClientCommand::MachineStop);
        cx.notify();
    }

    // ----- avatars

    /// Animation state for a project's avatar from what its agent is doing.
    pub fn avatar_state(&self, project_id: &str) -> crate::ui::avatars::AvatarState {
        use crate::ui::avatars::AvatarState;
        if self.project_is_streaming(project_id) {
            return AvatarState::Working;
        }
        match self.avatar_reply_until.get(project_id) {
            Some(until) if *until > now_ms() => AvatarState::Reply,
            _ => AvatarState::Idle,
        }
    }

    /// D92: the owner's leaf and disc-colour picks (`None` = automatic). Optimistic, then PATCH.
    pub fn set_avatar_leaf(
        &mut self,
        project_id: Id,
        leaf: Option<u8>,
        color: Option<u8>,
        cx: &mut Context<Self>,
    ) {
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == project_id) {
            p.avatar_leaf = leaf;
            p.avatar_leaf_color = color;
        }
        self.command(ClientCommand::SetAvatarLeaf {
            project_id,
            leaf,
            color,
        });
        cx.notify();
    }

    /// D92 amendment: the agent's picture from the avatar picker (a file to upload, or a
    /// generated key). The leaf stays stored underneath; the picture wins while set.
    pub fn set_project_image(
        &mut self,
        project_id: Id,
        source: crate::sync::PictureSource,
        cx: &mut Context<Self>,
    ) {
        self.command(ClientCommand::SetProjectImage { project_id, source });
        cx.notify();
    }

    /// Generate tab: ask the relay for a picture; the answer lands in `avatar_generated`.
    pub fn generate_image(
        &mut self,
        target: crate::sync::PictureTarget,
        prompt: String,
        cx: &mut Context<Self>,
    ) {
        self.command(ClientCommand::GenerateImage { target, prompt });
        cx.notify();
    }

    /// A people group's picture from a key the relay already holds (the Generate tab's answer).
    pub fn set_group_image_key(&mut self, chat_id: Id, key: String, cx: &mut Context<Self>) {
        self.command(ClientCommand::SetGroupImageKey { chat_id, key });
        cx.notify();
    }

    /// A server's picture from the avatar picker (owner / admins).
    pub fn set_server_image(
        &mut self,
        server_id: Id,
        source: crate::sync::PictureSource,
        cx: &mut Context<Self>,
    ) {
        self.command(ClientCommand::SetServerImage { server_id, source });
        cx.notify();
    }

    /// A server's leaf picture (`None` = initials; drops the uploaded picture).
    pub fn set_server_leaf(
        &mut self,
        server_id: Id,
        pick: Option<crate::ui::avatars::LeafPick>,
        cx: &mut Context<Self>,
    ) {
        if let Some(s) = self.servers.iter_mut().find(|s| s.id == server_id) {
            s.avatar_leaf = pick.map(|l| l.leaf);
            s.avatar_leaf_color = pick.map(|l| l.color);
            s.image_key = None;
            s.image_url = None;
        }
        self.command(ClientCommand::SetServerLeaf { server_id, pick });
        cx.notify();
    }

    /// A group's leaf picture from the avatar picker (`None` = back to initials; also drops
    /// the uploaded image so the leaf shows).
    pub fn set_group_leaf(
        &mut self,
        chat_id: Id,
        pick: Option<crate::ui::avatars::LeafPick>,
        cx: &mut Context<Self>,
    ) {
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == chat_id) {
            p.avatar_leaf = pick.map(|l| l.leaf);
            p.avatar_leaf_color = pick.map(|l| l.color);
            p.image_url = None;
        }
        self.command(ClientCommand::SetGroupLeaf { chat_id, pick });
        cx.notify();
    }

    /// Open the avatar picker sheet for an agent or a group (the shell hosts it).
    pub fn open_avatar_picker(
        &mut self,
        target: crate::ui::avatar_picker::Target,
        tab: crate::ui::avatar_picker::Tab,
        cx: &mut Context<Self>,
    ) {
        self.avatar_picker_request = Some((target, tab));
        cx.notify();
    }

    /// Rig: the Upload tab's pick, exactly as the file panel would hand it over (the preview is
    /// fetched through the avatar cache as `file://…`).
    fn rig_upload(&mut self, path: &str) {
        let path = std::path::PathBuf::from(path.trim());
        if path.as_os_str().is_empty() {
            return;
        }
        self.want_avatar(crate::ui::avatar_picker::file_url(&path));
        self.avatar_rig_upload = Some(path);
    }

    /// The agent's profile page (which holds the tree + colour picker).
    /// A second click on the same agent folds the pane away.
    pub fn toggle_avatar_picker(&mut self, project_id: Id, cx: &mut Context<Self>) {
        self.toggle_profile(ProfilePage::Agent { project_id }, cx);
    }

    // ----- profile pages ------------------------------------------------------

    /// The one action behind every click on a person or an agent in the chat —
    /// a sender name, the disc beside a bubble, an @mention chip, the header
    /// pill, a status-row picture — and the header pill of a group or channel
    /// ([feedback profile-pane-toggle-mac-2026-10-26], Jacob: "clicking the
    /// agent or user in the chat should open their user profile right bar.
    /// Clicking it again should close it"):
    ///
    /// - nothing open → `page` opens in the right pane;
    /// - that same person / agent showing (whichever target opened it,
    ///   [`ProfilePage::same_subject`]) → the pane closes;
    /// - someone else showing → `page` replaces it in place (no back stack).
    pub fn toggle_profile(&mut self, page: ProfilePage, cx: &mut Context<Self>) {
        if self.profile_shows(&page) {
            self.close_profile(cx);
        } else {
            self.open_profile(page, cx);
        }
    }

    /// True while the pane's top page is `page`'s subject (so a second click
    /// on any target for it closes the pane).
    pub fn profile_shows(&self, page: &ProfilePage) -> bool {
        self.profile.as_ref().is_some_and(|p| p.same_subject(page))
    }

    /// Fill the pane with `page`, forgetting whatever was under it.
    pub fn open_profile(&mut self, page: ProfilePage, cx: &mut Context<Self>) {
        self.profile_stack.clear();
        self.show_profile(page, cx);
    }

    /// Open `page` on top of the current one (a sender tapped inside a group,
    /// a member from a group's pane, an agent's owner): the page underneath
    /// is kept with its tab so `‹` brings it back as it was. Callers that
    /// know the body's scroll set it on the pushed frame (`Shell::push_profile`).
    /// Same page again is a no-op; nothing open is a plain open.
    pub fn push_profile(&mut self, page: ProfilePage, cx: &mut Context<Self>) {
        let Some(under) = self.profile.clone() else {
            self.open_profile(page, cx);
            return;
        };
        if under == page {
            return;
        }
        let tab = self.profile_tab;
        self.profile_stack.push(ProfileFrame {
            page: under,
            tab,
            scroll: 0.0,
        });
        self.show_profile(page, cx);
    }

    /// `‹` in the pane header: back to the page underneath on its tab, returning
    /// the body scroll it had for the shell to restore; closes when none.
    pub fn back_profile(&mut self, cx: &mut Context<Self>) -> Option<f32> {
        match self.profile_stack.pop() {
            Some(frame) => {
                self.show_profile(frame.page, cx);
                self.profile_tab = frame.tab;
                Some(frame.scroll)
            }
            None => {
                self.close_profile(cx);
                None
            }
        }
    }

    fn show_profile(&mut self, page: ProfilePage, cx: &mut Context<Self>) {
        self.avatar_picker = match &page {
            ProfilePage::Agent { project_id } => Some(project_id.clone()),
            _ => None,
        };
        if let ProfilePage::Agent { project_id } = &page {
            // A connected agent's Details: a fresh "Connected · N s ago" (#2141).
            if self
                .project(project_id)
                .is_some_and(|p| p.outside.is_some())
            {
                self.refresh_outside_status(project_id.clone());
            }
            self.command(ClientCommand::ShellGrantsRefresh {
                project_id: project_id.clone(),
            });
        }
        if let ProfilePage::Server { server_id } | ProfilePage::Channel { server_id, .. } = &page {
            // Members only come with the detail route.
            self.command(ClientCommand::ServerFetch {
                server_id: server_id.clone(),
            });
        }
        if let ProfilePage::Agent { project_id } = &page {
            // The Details tab's live cards (routines) and the Computer tab's
            // box report ([feedback agent-pane-details-tab-2026-10-24]).
            if self
                .project(project_id)
                .is_some_and(|p| !p.is_people() && !p.is_guest())
            {
                self.command(ClientCommand::RoutinesRefresh {
                    project_id: project_id.clone(),
                });
                self.command(ClientCommand::MachineDebugRefresh);
            }
        }
        self.profile = Some(page);
        self.profile_opened = self.profile_opened.wrapping_add(1);
        self.profile_from_name = false;
        self.profile_tab = 0;
        self.agent_pick = None;
        self.profile_editing_description = false;
        self.profile_editing_name = false;
        self.renaming_project = None;
        cx.notify();
    }

    /// Settings header: save my display name and handle (`PATCH /me`); the
    /// relay answers with the account, errors arrive as a notice.
    pub fn update_profile(&mut self, name: String, handle: String, cx: &mut Context<Self>) {
        if let Some(a) = self.account.as_mut() {
            a.name = name.clone();
            a.login = handle.clone();
        }
        self.command(ClientCommand::UpdateProfile { name, handle });
        cx.notify();
    }

    pub fn set_profile_editing_description(&mut self, on: bool, cx: &mut Context<Self>) {
        self.profile_editing_description = on;
        cx.notify();
    }

    /// Start an inline rename of an owned agent (double-click on its name).
    pub fn start_renaming(&mut self, project_id: Id, cx: &mut Context<Self>) {
        let ok = self
            .projects
            .iter()
            .any(|p| p.id == project_id && !p.is_guest() && !p.is_people());
        if !ok {
            return;
        }
        self.renaming_project = Some(project_id);
        self.profile_editing_description = false;
        cx.notify();
    }

    pub fn cancel_renaming(&mut self, cx: &mut Context<Self>) {
        let was = self.renaming_project.take().is_some() || self.profile_editing_name;
        self.profile_editing_name = false;
        if was {
            cx.notify();
        }
    }

    /// Agent page: save the description (`""` clears) — optimistic, then PATCH.
    pub fn set_description(&mut self, project_id: Id, description: String, cx: &mut Context<Self>) {
        let description = description.trim().chars().take(500).collect::<String>();
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == project_id) {
            p.description = Some(description.clone()).filter(|d| !d.is_empty());
        }
        self.profile_editing_description = false;
        self.command(ClientCommand::SetDescription {
            project_id,
            description,
        });
        cx.notify();
    }

    /// Rename an owned agent — optimistic, then `PATCH /projects/{id} {name}`.
    pub fn set_project_name(&mut self, project_id: Id, name: String, cx: &mut Context<Self>) {
        let name = name.trim().chars().take(64).collect::<String>();
        if name.is_empty() {
            self.cancel_renaming(cx);
            return;
        }
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == project_id) {
            if p.name == name {
                self.renaming_project = None;
                self.profile_editing_name = false;
                cx.notify();
                return;
            }
            p.name = name.clone();
        }
        // People-group AgentMember.handle is the @mention slug; refresh it so
        // a mid-chat rename does not leave mentions pointed at the old name.
        let handle = Project::mention_slug(&name);
        for space in self.projects.iter_mut().filter(|p| p.is_people()) {
            if let Some(a) = space.agents.iter_mut().find(|a| a.project_id == project_id) {
                a.name = name.clone();
                a.handle = handle.clone();
            }
        }
        self.renaming_project = None;
        self.profile_editing_name = false;
        self.command(ClientCommand::SetProjectName { project_id, name });
        cx.notify();
    }

    /// Name a people group from its page (any human member may).
    pub fn set_group_title(&mut self, chat_id: Id, title: String, cx: &mut Context<Self>) {
        let title = title.trim().chars().take(64).collect::<String>();
        self.profile_editing_name = false;
        if title.is_empty() {
            cx.notify();
            return;
        }
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == chat_id) {
            if p.name == title {
                cx.notify();
                return;
            }
            p.name = title.clone();
        }
        for tabs in self.chats.values_mut() {
            for c in tabs.iter_mut().filter(|c| c.id == chat_id) {
                c.title = title.clone();
            }
        }
        self.command(ClientCommand::RenameGroup { chat_id, title });
        cx.notify();
    }

    /// A picked file becomes my picture everywhere (`PUT /me/avatar`).
    pub fn set_my_avatar(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        self.command(ClientCommand::SetMyAvatar { path });
        cx.notify();
    }

    /// The avatar picker's generated preview becomes my picture (`PUT /me/avatar {image_key}`).
    pub fn set_my_avatar_key(&mut self, key: String, cx: &mut Context<Self>) {
        self.command(ClientCommand::SetMyAvatarKey { key });
        cx.notify();
    }

    /// Back to the GitHub picture.
    pub fn remove_my_avatar(&mut self, cx: &mut Context<Self>) {
        self.command(ClientCommand::RemoveMyAvatar);
        cx.notify();
    }

    /// My picture is an Arbos override (the relay's `/avatars/…`), not GitHub's.
    pub fn has_custom_avatar(&self) -> bool {
        self.account.as_ref().is_some_and(|a| {
            a.avatar_url.contains("/avatars/") || a.avatar_url.starts_with("fixture://avatar/me-")
        })
    }

    /// A picked file becomes the group's picture.
    pub fn set_group_image(
        &mut self,
        chat_id: Id,
        path: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        self.command(ClientCommand::SetGroupImage { chat_id, path });
        cx.notify();
    }

    /// Closes the sheet; `true` when one was open. An edit in progress on the
    /// agent page (name, description) is dropped with it.
    pub fn close_profile(&mut self, cx: &mut Context<Self>) -> bool {
        self.profile_stack.clear();
        self.avatar_picker = None;
        self.profile_from_name = false;
        self.agent_pick = None;
        let was = self.profile.take().is_some();
        if was {
            self.profile_editing_name = false;
            self.profile_editing_description = false;
            cx.notify();
        }
        was
    }

    pub fn set_profile_tab(&mut self, tab: usize, cx: &mut Context<Self>) {
        self.profile_tab = tab;
        // The agent pane's Computer tab shows the box as of now.
        if tab == crate::ui::profiles::agent::TAB_COMPUTER
            && matches!(self.profile, Some(ProfilePage::Agent { .. }))
        {
            self.command(ClientCommand::MachineDebugRefresh);
        }
        cx.notify();
    }

    /// Agent page: drop one settings row down (`None` folds them all).
    pub fn set_agent_pick(&mut self, pick: Option<AgentPick>, cx: &mut Context<Self>) {
        if self.agent_pick != pick {
            self.agent_pick = pick;
            cx.notify();
        }
    }

    /// Muted: the relay's `me.muted` for a people chat (synced across devices),
    /// else the local list (projects have no relay-side setting yet).
    pub fn is_muted(&self, space_id: &str) -> bool {
        self.project(space_id).is_some_and(|p| p.muted) || self.muted.contains(space_id)
    }

    /// Sidebar archive (Jacob 2026-10-23): is this space (agent, people chat
    /// or server) hidden from my list?
    pub fn is_space_archived(&self, space_id: &str) -> bool {
        self.project(space_id)
            .map(|p| p.archived_at.is_some())
            .or_else(|| {
                self.servers
                    .iter()
                    .find(|s| s.id == space_id)
                    .map(|s| s.archived_at.is_some())
            })
            .unwrap_or(false)
    }

    /// Hidden spaces, newest first; what the "Hidden chats · N" footer counts.
    pub fn archived_space_count(&self) -> usize {
        self.projects
            .iter()
            .filter(|p| p.archived_at.is_some())
            .count()
            + self
                .servers
                .iter()
                .filter(|s| s.archived_at.is_some())
                .count()
    }

    // ---- Sidebar pins ([feedback sidebar-pinning-mac-2026-10-28]) ------------------------

    /// The relay's row key for a sidebar space, or `None` for something the list does not
    /// show: `agent:` for an agent (own or shared), `chat:` for a DM or a people group
    /// (their space id is the chat id), `server:` for a server.
    pub fn pin_key(&self, space_id: &str) -> Option<String> {
        if let Some(p) = self.projects.iter().find(|p| p.id == space_id) {
            return Some(if p.is_people() {
                format!("chat:{space_id}")
            } else {
                format!("agent:{space_id}")
            });
        }
        self.servers
            .iter()
            .any(|s| s.id == space_id)
            .then(|| format!("server:{space_id}"))
    }

    /// The space a pin names, if it is in my list today.
    pub fn pinned_space_id(key: &str) -> Option<&str> {
        key.split_once(':')
            .filter(|(kind, id)| matches!(*kind, "agent" | "chat" | "server") && !id.is_empty())
            .map(|(_, id)| id)
    }

    pub fn is_pinned(&self, space_id: &str) -> bool {
        self.pin_index(space_id).is_some()
    }

    /// Position in the Pinned section (0 = top), or `None` when not pinned.
    pub fn pin_index(&self, space_id: &str) -> Option<usize> {
        let key = self.pin_key(space_id)?;
        self.pins.iter().position(|k| *k == key)
    }

    /// Pin (a new pin lands at the top, the iPhone's rule) or unpin a sidebar row, on
    /// every device. At the cap the pin is refused with one sentence; archived rows come
    /// back first (a pin is a row you see).
    pub fn set_pinned(&mut self, space_id: Id, pinned: bool, cx: &mut Context<Self>) {
        let Some(key) = self.pin_key(&space_id) else {
            return;
        };
        let mut pins = self.pins.clone();
        pins.retain(|k| *k != key);
        if pinned {
            if pins.len() >= self.pin_cap {
                self.push_failure(
                    UserError::of(
                        "pin the chat",
                        ErrorClass::Refused {
                            message: Some(format!("You can pin up to {} chats.", self.pin_cap)),
                        },
                        "",
                    ),
                    None,
                );
                cx.notify();
                return;
            }
            if self.is_space_archived(&space_id) {
                self.set_space_archived(space_id.clone(), false, cx);
            }
            pins.insert(0, key);
        }
        self.store_pins(pins, cx);
    }

    /// Drop of a pin drag: `shown` is the Pinned section as the list drew it (the pinned
    /// rows the filter admits, in their new order). Those pins take that order among
    /// themselves; pins the filter hid keep their places.
    pub fn reorder_pins(&mut self, shown: &[Id], cx: &mut Context<Self>) {
        let shown_keys: Vec<String> = shown
            .iter()
            .filter_map(|id| self.pin_key(id))
            .filter(|k| self.pins.contains(k))
            .collect();
        let mut next = shown_keys.iter().cloned();
        let pins: Vec<String> = self
            .pins
            .iter()
            .map(|k| {
                if shown_keys.contains(k) {
                    next.next().unwrap_or_else(|| k.clone())
                } else {
                    k.clone()
                }
            })
            .collect();
        self.store_pins(pins, cx);
    }

    fn unpin_if_pinned(&mut self, space_id: &str) {
        let Some(key) = self.pin_key(space_id) else {
            return;
        };
        if self.pins.contains(&key) {
            self.pins.retain(|k| *k != key);
            self.command(ClientCommand::SetPins {
                pins: self.pins.clone(),
            });
        }
    }

    /// The new list, shown at once and sent to the relay (its answer is the truth).
    fn store_pins(&mut self, pins: Vec<String>, cx: &mut Context<Self>) {
        if pins == self.pins {
            return;
        }
        self.pins = pins.clone();
        self.command(ClientCommand::SetPins { pins });
        cx.notify();
    }

    /// Archive (hide) or unarchive a sidebar space for me, on every device.
    /// Nothing is deleted; an archived agent keeps running its routines. A
    /// people chat goes through the chat archive the relay already has.
    pub fn set_space_archived(&mut self, space_id: Id, archived: bool, cx: &mut Context<Self>) {
        let at = archived.then(now_ms);
        if archived {
            // A hidden row is not a pinned row (the iPhone's rule).
            self.unpin_if_pinned(&space_id);
        }
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == space_id) {
            p.archived_at = at;
            let people = p.is_people();
            if archived && self.selected_project.as_deref() == Some(space_id.as_str()) {
                self.selected_project = None;
                self.select_next_if_none(cx);
            }
            if people {
                self.command(ClientCommand::ArchiveChat {
                    chat_id: space_id,
                    archived,
                });
            } else {
                self.command(ClientCommand::ArchiveSpace {
                    space_id,
                    server: false,
                    archived,
                });
            }
            cx.notify();
            return;
        }
        if let Some(s) = self.servers.iter_mut().find(|s| s.id == space_id) {
            s.archived_at = at;
            if archived && self.open_server.as_deref() == Some(space_id.as_str()) {
                self.open_server = None;
                self.selected_channel = None;
                self.select_next_if_none(cx);
            }
            self.command(ClientCommand::ArchiveSpace {
                space_id,
                server: true,
                archived,
            });
            cx.notify();
        }
    }

    /// A hidden space comes back when something new arrives in it (Telegram).
    fn unarchive_on_activity(&mut self, chat_id: &str, cx: &mut Context<Self>) {
        let Some(space_id) = self
            .project_of_chat(chat_id)
            .filter(|p| p.archived_at.is_some())
            .map(|p| p.id.clone())
        else {
            return;
        };
        self.set_space_archived(space_id, false, cx);
    }

    pub fn toggle_mute(&mut self, space_id: Id, cx: &mut Context<Self>) {
        let now_muted = !self.is_muted(&space_id);
        let people = self.project(&space_id).is_some_and(|p| p.is_people());
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == space_id) {
            p.muted = now_muted;
        }
        if now_muted {
            self.muted.insert(space_id.clone());
        } else {
            self.muted.remove(&space_id);
        }
        save_muted(&self.muted);
        if people {
            self.command(ClientCommand::SetMuted {
                space_id,
                muted: now_muted,
            });
        }
        cx.notify();
    }

    /// Call a person (their DM) or an agent (its chat; Calls Phase 2). The
    /// panes no longer offer it (2026-10-21); the composer's call button does.
    #[allow(dead_code)]
    pub fn start_call(&mut self, target: CallTarget, video: bool, cx: &mut Context<Self>) {
        let kind = if video {
            CallKind::Video
        } else {
            CallKind::Voice
        };
        match target {
            CallTarget::User(handle) => {
                let dm = self
                    .people_spaces()
                    .find(|p| p.person().map(|x| x.handle == handle).unwrap_or(false))
                    .map(|p| p.id.clone());
                match dm {
                    Some(chat_id) => {
                        self.select_project(chat_id, cx);
                        self.start_call_in_chat(kind, cx);
                    }
                    None => {
                        self.push_notice(format!("Start a chat with @{handle} first, then call."));
                        cx.notify();
                    }
                }
            }
            CallTarget::Agent(project_id) => {
                // Calls Phase 2: a call in the agent's chat; the voice worker joins as the agent.
                self.close_profile(cx);
                self.select_project(project_id, cx);
                self.start_call_in_chat(kind, cx);
            }
        }
    }

    /// A person by handle, from any space's member list or the directory.
    pub fn person_by_handle(&self, handle: &str) -> Option<Person> {
        self.projects
            .iter()
            .flat_map(|p| p.people.iter())
            // A shared agent's owner (API §9 `owner`), who need not be in any member list.
            .chain(self.projects.iter().filter_map(|p| p.owner_person.as_ref()))
            .chain(self.people_results.iter())
            .find(|p| p.handle == handle)
            .cloned()
    }

    pub fn machine_restart(&mut self, cx: &mut Context<Self>) {
        self.command(ClientCommand::MachineRestart);
        cx.notify();
    }

    pub fn toggle_show_details(&mut self, cx: &mut Context<Self>) {
        self.show_details = !self.show_details;
        if let Some(chat_id) = self.active_chat_id() {
            cx.emit(AppEvent::TranscriptChanged { chat_id });
        }
        cx.notify();
    }

    pub fn open_details(&mut self, target: DetailsTarget, cx: &mut Context<Self>) {
        self.details_open = Some(target);
        cx.notify();
    }

    pub fn close_details(&mut self, cx: &mut Context<Self>) {
        self.details_open = None;
        cx.notify();
    }

    /// The picture a chat bubble shows for blob `key`: the trimmed copy when
    /// a uniform border was cut, else the full thumbnail.
    pub fn card_thumbnail(&self, key: &str) -> Option<Arc<RenderImage>> {
        self.thumbnails_trimmed
            .get(key)
            .or_else(|| self.thumbnails.get(key))
            .cloned()
    }

    // ----- media viewer (feedback media-viewer-2026-10-23) -----------------

    /// Every picture and video of `chat_id` in transcript order — the album the
    /// viewer pages through. Files are not in it: a file opens alone.
    pub fn media_album(&self, chat_id: &str) -> Vec<ViewerItem> {
        let mut out = Vec::new();
        for m in self.messages_of(chat_id) {
            for part in &m.parts {
                let Part::Attachment { id, attachment } = part else {
                    continue;
                };
                if !matches!(
                    attachment.kind(),
                    AttachmentKind::Image | AttachmentKind::Video
                ) {
                    continue;
                }
                out.push(ViewerItem {
                    attachment: attachment.clone(),
                    by: self.viewer_sender_name(m),
                    when: m.created_at,
                    chat_id: chat_id.to_owned(),
                    part_id: id.clone(),
                    picture_url: None,
                });
            }
        }
        out
    }

    /// Who shared it, for the viewer's caption: name, else `@handle`, else "You" / the agent.
    pub fn viewer_sender_name(&self, m: &Message) -> String {
        if let Some(n) = m.sender_name.as_deref().filter(|n| !n.is_empty()) {
            return n.to_string();
        }
        if let Some(h) = m.sender_handle.as_deref().filter(|h| !h.is_empty()) {
            return self
                .person_by_handle(h)
                .map(|p| p.display_name())
                .unwrap_or_else(|| format!("@{h}"));
        }
        if self.is_mine(m) {
            return "You".into();
        }
        m.sender
            .strip_prefix("agent:")
            .or_else(|| m.sender.strip_prefix("project:"))
            .and_then(|pid| self.project(pid))
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Agent".into())
    }

    /// Open the viewer on one attachment of a chat: pictures and videos open
    /// inside the chat's album at their place; a file opens on its own.
    pub fn open_media_in_chat(
        &mut self,
        chat_id: &str,
        message_id: &str,
        part_id: &str,
        cx: &mut Context<Self>,
    ) {
        let album = self.media_album(chat_id);
        if let Some(ix) = album
            .iter()
            .position(|i| i.part_id == part_id && i.chat_id == chat_id)
        {
            self.open_media_viewer(album, ix, cx);
            return;
        }
        let Some(m) = self
            .messages_of(chat_id)
            .iter()
            .find(|m| m.id == message_id)
        else {
            return;
        };
        let Some(attachment) = m.parts.iter().find_map(|p| match p {
            Part::Attachment { id, attachment } if id == part_id => Some(attachment.clone()),
            _ => None,
        }) else {
            return;
        };
        let item = ViewerItem {
            attachment,
            by: self.viewer_sender_name(m),
            when: m.created_at,
            chat_id: chat_id.to_owned(),
            part_id: part_id.to_owned(),
            picture_url: None,
        };
        self.open_media_viewer(vec![item], 0, cx);
    }

    /// A profile picture full-size in the media viewer (Jacob, 2026-10-25: tap the hero on any
    /// pane to view it). Same viewer, same close gestures; the bytes come from the avatar cache
    /// (fetched now when missing). Nothing opens for an empty URL (letter / leaf avatars).
    pub fn open_picture_viewer(&mut self, name: &str, url: &str, cx: &mut Context<Self>) {
        if url.trim().is_empty() {
            return;
        }
        self.want_avatar(url.to_owned());
        self.media_viewer = Some(MediaViewer {
            items: vec![ViewerItem::picture(name, url)],
            index: 0,
            slot: self.selected_project.clone().unwrap_or_default(),
            forward_open: false,
            busy: None,
            playing: None,
        });
        self.media_viewer_generation += 1;
        cx.notify();
    }

    /// Show `items[index]`. Fetches the picture when no thumbnail is cached yet.
    pub fn open_media_viewer(
        &mut self,
        items: Vec<ViewerItem>,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        if items.is_empty() {
            return;
        }
        let index = index.min(items.len() - 1);
        // The slot the chat's blobs live in: its project, or the people chat
        // itself (`space_of_chat`); a channel row falls back to the selected space.
        let slot = {
            let chat_id = &items[index].chat_id;
            let known = self
                .chats
                .values()
                .any(|tabs| tabs.iter().any(|c| &c.id == chat_id));
            if known {
                self.space_of_chat(chat_id)
            } else {
                match self.selected_project.clone() {
                    Some(p) => p,
                    None => return,
                }
            }
        };
        self.media_viewer = Some(MediaViewer {
            items,
            index,
            slot,
            forward_open: false,
            busy: None,
            playing: None,
        });
        self.media_viewer_generation += 1;
        self.ensure_viewer_pictures();
        cx.notify();
    }

    pub fn close_media_viewer(&mut self, cx: &mut Context<Self>) {
        self.media_viewer = None;
        cx.notify();
    }

    /// ← / → (or the chevrons): move through the album, clamped at the ends.
    pub fn media_viewer_step(&mut self, delta: i32, cx: &mut Context<Self>) {
        let Some(viewer) = self.media_viewer.as_mut() else {
            return;
        };
        if viewer.forward_open {
            return;
        }
        let last = viewer.items.len() as i32 - 1;
        let next = (viewer.index as i32 + delta).clamp(0, last.max(0));
        if next as usize != viewer.index {
            viewer.playing = None;
            viewer.index = next as usize;
            self.ensure_viewer_pictures();
            cx.notify();
        }
    }

    /// Ask for the current item's picture and its neighbours' so paging is instant.
    fn ensure_viewer_pictures(&mut self) {
        let Some(viewer) = self.media_viewer.as_ref() else {
            return;
        };
        let slot = viewer.slot.clone();
        let keys: Vec<String> = [viewer.index.wrapping_sub(1), viewer.index, viewer.index + 1]
            .into_iter()
            .filter_map(|ix| viewer.items.get(ix))
            .filter(|i| i.attachment.kind() == AttachmentKind::Image)
            .filter_map(|i| i.attachment.blob_ref.clone())
            .filter(|k| !k.starts_with("local:"))
            .collect();
        for key in keys {
            if self.thumbnails.contains_key(&key) {
                continue;
            }
            // A card that said "No preview" gets one more try when it is opened.
            self.thumbnails_failed.remove(&key);
            self.thumbnails_requested.insert(key.clone());
            self.command(ClientCommand::FetchThumbnail {
                project_id: slot.clone(),
                key,
            });
        }
    }

    /// Viewer → Download: `~/Downloads/<name>`, then reveal (the card's button does the same).
    pub fn media_viewer_download(&mut self, cx: &mut Context<Self>) {
        let Some(viewer) = self.media_viewer.as_ref() else {
            return;
        };
        let Some(item) = viewer.items.get(viewer.index) else {
            return;
        };
        let Some(key) = item.attachment.blob_ref.clone() else {
            return;
        };
        if key.starts_with("local:") {
            self.push_notice("Still uploading — try again in a moment");
            cx.notify();
            return;
        }
        let name = item.attachment.name.clone();
        let project_id = viewer.slot.clone();
        self.command(ClientCommand::DownloadAttachment {
            project_id,
            key,
            name,
        });
        cx.notify();
    }

    /// A Library row's Copy ([feedback copy-image-messages-mac-2026-10-27]): the
    /// picture or file of that row, from the slot its chat's blobs live in (its
    /// project, or the people chat itself; a channel row falls back to the selected space).
    pub fn copy_viewer_item(&mut self, item: &ViewerItem, cx: &mut Context<Self>) {
        let known = self
            .chats
            .values()
            .any(|tabs| tabs.iter().any(|c| c.id == item.chat_id));
        let slot = if known {
            self.space_of_chat(&item.chat_id)
        } else {
            match self.selected_project.clone() {
                Some(p) => p,
                None => return,
            }
        };
        let attachment = item.attachment.clone();
        self.copy_attachment(slot, &attachment, cx);
    }

    /// Viewer → Copy (the pill, or ⌘C while the viewer is up): the open picture or
    /// file on the pasteboard, original bytes under their own type
    /// ([feedback copy-image-messages-mac-2026-10-27]).
    pub fn media_viewer_copy(&mut self, cx: &mut Context<Self>) {
        let Some(viewer) = self.media_viewer.as_ref() else {
            return;
        };
        let Some(item) = viewer.items.get(viewer.index) else {
            return;
        };
        if item.picture_url.is_some() || viewer.forward_open {
            return;
        }
        let (slot, attachment) = (viewer.slot.clone(), item.attachment.clone());
        self.copy_attachment(slot, &attachment, cx);
    }

    /// Viewer → Play / Quick Look: fetch the bytes to a temp file. Files open
    /// in Quick Look; videos play inline in the viewer on macOS (AVPlayerView)
    /// instead of handing off to QuickTime ([feedback
    /// app-01M6147S9TPNBQA7KSDEY0K8C4]).
    pub fn media_viewer_open_external(&mut self, cx: &mut Context<Self>) {
        let Some(viewer) = self.media_viewer.as_mut() else {
            return;
        };
        let Some(item) = viewer.items.get(viewer.index) else {
            return;
        };
        let Some(key) = item.attachment.blob_ref.clone() else {
            return;
        };
        if key.starts_with("local:") || viewer.busy.is_some() {
            return;
        }
        if viewer.playing.is_some() && item.attachment.kind() == AttachmentKind::Video {
            return;
        }
        let quick_look = item.attachment.kind() != AttachmentKind::Video;
        let name = item.attachment.name.clone();
        let project_id = viewer.slot.clone();
        viewer.busy = Some(key.clone());
        viewer.playing = None;
        self.command(ClientCommand::OpenAttachment {
            project_id,
            key,
            name,
            quick_look,
        });
        cx.notify();
    }

    pub fn media_viewer_toggle_forward(&mut self, cx: &mut Context<Self>) {
        if let Some(viewer) = self.media_viewer.as_mut() {
            viewer.forward_open = !viewer.forward_open;
            cx.notify();
        }
    }

    /// The chats the Forward picker offers: every DM, agent and group on the
    /// rail, the current chat included, filtered by `query`, most recent first.
    /// Channels are left out — they carry no attachments yet (Matrix `m.text`).
    pub fn forward_targets(&self, query: &str) -> Vec<ForwardTarget> {
        let out: Vec<ForwardTarget> = self
            .projects
            .iter()
            .filter_map(|p| {
                let chat_id = self.primary_chat(&p.id).map(|c| c.id.clone())?;
                // A DM's relay name is the other side's handle; the row (and the
                // matcher) want the display name, with the @handle underneath.
                let name = match p.person() {
                    Some(person) if p.kind == crate::model::SpaceKind::Dm => person.display_name(),
                    _ => p.name.clone(),
                };
                let subtitle = match p.kind {
                    crate::model::SpaceKind::Dm => p
                        .person()
                        .map(|q| format!("@{}", q.handle))
                        .unwrap_or_else(|| "Direct message".into()),
                    crate::model::SpaceKind::Group => {
                        let n = p.people.len() + 1 + p.agents.len();
                        format!("Group · {n} members")
                    }
                    _ => "Agent".into(),
                };
                Some(ForwardTarget {
                    space_id: p.id.clone(),
                    chat_id,
                    name,
                    subtitle,
                    last_activity: self
                        .tabs(&p.id)
                        .iter()
                        .map(|c| c.last_message_at)
                        .max()
                        .unwrap_or(0),
                })
            })
            .collect();
        filter_forward_targets(out, query)
    }

    /// Forward the open item to `target`: the row lands in that chat at once
    /// (optimistic, same thumbnail), the source copies the blob into the
    /// destination's slot and writes the message with "Forwarded from <name>".
    pub fn forward_media_to(&mut self, target: ForwardTarget, cx: &mut Context<Self>) {
        let Some(viewer) = self.media_viewer.as_mut() else {
            return;
        };
        let Some(item) = viewer.items.get(viewer.index).cloned() else {
            return;
        };
        let Some(key) = item.attachment.blob_ref.clone() else {
            return;
        };
        if key.starts_with("local:") {
            self.push_notice("Still uploading — try again in a moment");
            cx.notify();
            return;
        }
        viewer.forward_open = false;
        let from_slot = viewer.slot.clone();
        let message_id = new_id();
        let text = forwarded_from_text(&item.by);
        let mut attachment = item.attachment.clone();
        attachment.title = None;
        let parts = vec![
            Part::Attachment {
                id: new_id(),
                attachment: attachment.clone(),
            },
            Part::Text {
                id: new_id(),
                text: text.clone(),
            },
        ];
        self.messages
            .entry(target.chat_id.clone())
            .or_default()
            .push(Message {
                id: message_id.clone(),
                chat_id: target.chat_id.clone(),
                role: Role::User,
                sender: "user".into(),
                status: MessageStatus::Queued,
                turn: None,
                decision: None,
                subagent_id: None,
                subagent_name: None,
                handoff_messages: None,
                notice: None,
                worker: false,
                job: None,
                hidden: false,
                agent_reply_to: None,
                reactions: Default::default(),
                in_reply_to: None,
                routine_name: None,
                sender_name: None,
                sender_icon: None,
                sender_handle: None,
                sender_avatar: None,
                sender_avatar_color: None,
                mentions: Vec::new(),
                reply_to: None,
                forwarded_from: None,
                reply_ref: None,
                asked_by_handle: None,
                owner_handle: None,
                usage: None,
                return_contract: None,
                waiting_user: false,
                waiting_approval: false,
                model: None,
                effort: None,
                created_at: now_ms(),
                completed_at: None,
                call_id: None,
                activity: None,
                pending: true,
                parts,
                hidden_parts: Vec::new(),
                greeting: false,
                preview: None,
            });
        self.command(ClientCommand::Forward {
            chat_id: target.chat_id.clone(),
            message_id,
            from_slot,
            attachment,
            text,
        });
        self.push_notice(format!("Forwarded to {}", target.name));
        cx.emit(AppEvent::TranscriptChanged {
            chat_id: target.chat_id,
        });
        cx.notify();
    }

    // ----- bubble menu → Forward (feedback app-01M5MXRZ7609S70P9S9P2RZDDF) -----

    /// Who a message is from, for "Forwarded from <name>": the row's sender
    /// name, else the person's display name, my own name for my bubbles, else
    /// the agent's name.
    pub fn forward_sender_name(&self, m: &Message) -> String {
        if self.is_mine(m) {
            if let Some(a) = self.account.as_ref() {
                let name = if a.name.trim().is_empty() {
                    a.login.clone()
                } else {
                    a.name.clone()
                };
                if !name.is_empty() {
                    return name;
                }
            }
        }
        self.viewer_sender_name(m)
    }

    /// Open the "Forward to" picker for a bubble (the media viewer's picker,
    /// over the chat).
    pub fn open_forward_message(&mut self, chat_id: Id, message_id: Id, cx: &mut Context<Self>) {
        let Some(message) = self
            .messages
            .get(&chat_id)
            .and_then(|ms| ms.iter().find(|m| m.id == message_id))
        else {
            return;
        };
        // Words, pictures, files, charts — any of them is enough; a picture still
        // uploading is not on the relay yet, so there is nothing to copy.
        if message.body().is_none() {
            self.push_notice(if message.has_uploading_attachment() {
                "Still uploading — try again in a moment"
            } else {
                "Nothing to forward"
            });
            cx.notify();
            return;
        }
        // Forwarding a forwarded message keeps the original author (Telegram), not
        // whoever forwarded it last.
        let by = message
            .forwarded_from
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| self.forward_sender_name(message));
        self.forward_message = Some(ForwardMessage {
            chat_id,
            message_id,
            by,
        });
        cx.notify();
    }

    pub fn close_forward_message(&mut self, cx: &mut Context<Self>) {
        if self.forward_message.take().is_some() {
            cx.notify();
        }
    }

    /// Forward the picked message to `target` as a new message of mine there
    /// (no reply quote): its text, its pictures / files / videos (each blob
    /// copied into the destination's slot by the source) and its charts, all
    /// carrying `forwardedFrom: <name>` so every client draws the "Forwarded
    /// from <name>" header on the bubble
    /// ([feedback forward-attachments-mac-2026-10-27]: the image and its
    /// caption used to arrive as the caption alone).
    pub fn forward_message_to(&mut self, target: ForwardTarget, cx: &mut Context<Self>) {
        let Some(source) = self.forward_message.take() else {
            return;
        };
        let Some(payload) = self
            .messages
            .get(&source.chat_id)
            .and_then(|ms| ms.iter().find(|m| m.id == source.message_id))
            .and_then(Message::body)
        else {
            cx.notify();
            return;
        };
        let from_slot = self.space_of_chat(&source.chat_id);
        let message_id = new_id();
        let parts = forward_optimistic_parts(&payload);
        self.messages
            .entry(target.chat_id.clone())
            .or_default()
            .push(Message {
                id: message_id.clone(),
                chat_id: target.chat_id.clone(),
                role: Role::User,
                sender: "user".into(),
                status: MessageStatus::Queued,
                turn: None,
                decision: None,
                subagent_id: None,
                subagent_name: None,
                handoff_messages: None,
                notice: None,
                worker: false,
                job: None,
                hidden: false,
                agent_reply_to: None,
                reactions: Default::default(),
                in_reply_to: None,
                routine_name: None,
                sender_name: None,
                sender_icon: None,
                sender_handle: None,
                sender_avatar: None,
                sender_avatar_color: None,
                mentions: Vec::new(),
                reply_to: None,
                forwarded_from: Some(source.by.clone()),
                reply_ref: None,
                asked_by_handle: None,
                owner_handle: None,
                usage: None,
                return_contract: None,
                waiting_user: false,
                waiting_approval: false,
                model: None,
                effort: None,
                created_at: now_ms(),
                completed_at: None,
                call_id: None,
                activity: None,
                pending: true,
                parts,
                hidden_parts: Vec::new(),
                greeting: false,
                preview: None,
            });
        self.command(ClientCommand::ForwardMessage {
            chat_id: target.chat_id.clone(),
            message_id,
            from_slot,
            text: payload.text,
            attachments: payload.attachments,
            charts: payload.charts,
            forwarded_from: source.by,
        });
        self.push_notice(format!("Forwarded to {}", target.name));
        cx.emit(AppEvent::TranscriptChanged {
            chat_id: target.chat_id,
        });
        cx.notify();
    }

    // ----- system notifications (D49) --------------------------------------

    fn notify_gate(&self, project_id: &str, chat_id: Option<&str>) -> Gate {
        let project_shown = self.selected_project.as_deref() == Some(project_id);
        let chat_shown = match chat_id {
            Some(id) => self.active_chat_id().as_deref() == Some(id),
            None => true,
        };
        Gate {
            streaming: self.project_is_streaming(project_id),
            // Muted spaces are treated as "already seen": dropped, not queued.
            // Honour the relay's per-chat muted (people) as well as muted.json.
            visible: (self.window_active && project_shown && chat_shown)
                || self.is_muted(project_id),
        }
    }

    /// Is `chat_id` the chat on screen in a focused window?
    fn chat_visible_in_active_window(&self, chat_id: &str) -> bool {
        self.window_active && self.active_chat_id().as_deref() == Some(chat_id)
    }

    /// Apply what a batch of arrivals does to the chat's read state: one unread lights the
    /// dot; otherwise, content seen on screen advances the user's read cursor on the relay
    /// (`MarkRead` → `PUT /chats/{id}/read`) so no device of theirs shows it unread later.
    fn apply_arrival_read(&mut self, chat_id: &str, effects: &[ArrivalRead]) {
        if effects.contains(&ArrivalRead::MarkUnread) {
            self.mark_chat_unread(chat_id);
        } else if effects.contains(&ArrivalRead::AdvanceCursor) {
            self.command(ClientCommand::MarkRead {
                chat_id: chat_id.to_owned(),
            });
        }
    }

    /// Light the unread dot (and Dock badge) until the next refresh confirms.
    /// Until `GET /me/unread` returns, count at least one missed message (same as iOS).
    fn mark_chat_unread(&mut self, chat_id: &str) {
        if let Some(chat) = self
            .chats
            .values_mut()
            .flat_map(|v| v.iter_mut())
            .find(|c| c.id == chat_id)
        {
            chat.unread = true;
            chat.unread_count = chat.unread_count.max(1);
        }
        self.sync_badge();
    }

    fn notify(&mut self, project_id: Id, chat_id: Option<&str>, kind: NotifyKind, detail: String) {
        self.notify_from(project_id, chat_id, kind, detail, None, None);
    }

    /// Like [`Self::notify`], as a chat message: `sender` titles the banner
    /// and `image` (the sender's chat picture) is attached.
    fn notify_from(
        &mut self,
        project_id: Id,
        chat_id: Option<&str>,
        kind: NotifyKind,
        detail: String,
        sender: Option<String>,
        image: Option<PathBuf>,
    ) {
        let Some(project) = self.project(&project_id) else {
            return;
        };
        // A message in a group (people group, shared project chat): the banner is titled
        // with the group and the sender moves into the body — "Arbonites" / "Doug: Five." —
        // with the group's picture; the same shape as the iPhone and the relay's APNs payload
        // (Jacob 2026-10-27: "cannot tell which group a message came from"). A DM keeps the
        // sender as the title.
        let group = (kind.is_message() && project.is_groupish() && sender.is_some()).then(|| {
            (
                project.name.clone(),
                project
                    .image_url
                    .as_deref()
                    .and_then(|u| self.person_notification_png(u))
                    .or_else(|| {
                        crate::ui::avatars::notification_png(
                            &project.id,
                            &project.name,
                            project.avatar_color_index,
                            crate::ui::theme::system_is_dark(),
                        )
                    }),
            )
        });
        let (detail, sender, image) = match group {
            Some((group_name, group_image)) => {
                let (title, body) = notify::group_frame(
                    &group_name,
                    sender.as_deref().unwrap_or_default(),
                    &detail,
                );
                (body, Some(title), group_image.or(image))
            }
            None => (detail, sender, image),
        };
        let mut event = NotifyEvent::new(project.id.clone(), project.name.clone(), kind, detail);
        event.chat_id = chat_id.map(str::to_string);
        event.sender = sender;
        event.image = image;
        let gate = self.notify_gate(&project_id, chat_id);
        self.notifier.offer(event, now_ms(), gate);
    }

    /// A person's GitHub avatar as a round PNG for the notification, from the
    /// decoded image the rail already holds (`None` until it has loaded).
    fn person_notification_png(&self, url: &str) -> Option<PathBuf> {
        let img = self.avatars.get(url)?.as_ref()?;
        let bytes = img.as_bytes(0)?;
        let size = img.size(0);
        let (w, h) = (size.width.0 as u32, size.height.0 as u32);
        if w == 0 || h == 0 || bytes.len() < (w * h * 4) as usize {
            return None;
        }
        let mut rgba = bytes[..(w * h * 4) as usize].to_vec();
        let c = (w.min(h) as f32 - 1.0) / 2.0;
        let r = w.min(h) as f32 / 2.0;
        for (i, px) in rgba.chunks_exact_mut(4).enumerate() {
            px.swap(0, 2);
            let (x, y) = ((i as u32 % w) as f32, (i as u32 / w) as f32);
            let d = ((x - c).powi(2) + (y - c).powi(2)).sqrt();
            let cover = (r - d + 0.5).clamp(0.0, 1.0);
            px[3] = (px[3] as f32 * cover) as u8;
        }
        let key = format!("person-{:016x}", fnv1a64(url));
        notify::image_file(&key, w, h, &rgba)
    }

    /// The agent behind a reply: its name and chat image (a shared agent's
    /// own tree and colour when the message or member list carries them,
    /// else this project's).
    fn agent_identity(&self, project_id: &str, message: &Message) -> (String, Option<PathBuf>) {
        let project = self.projects.iter().find(|p| p.id == project_id);
        // `agent:<pid>` in a people chat → that member; `project:<pid>` in a
        // group chat → that project of mine.
        // (seed id, name, gradient override) of whoever wrote it.
        let shared_agent = message
            .sender
            .strip_prefix("agent:")
            .and_then(|pid| {
                project
                    .and_then(|p| p.agents.iter().find(|a| a.project_id == pid))
                    .map(|a| (a.project_id.clone(), a.name.clone(), a.avatar_color_index))
            })
            .or_else(|| {
                let pid = message.sender.strip_prefix("project:")?;
                let other = self.projects.iter().find(|p| p.id == pid)?;
                Some((
                    other.id.clone(),
                    other.name.clone(),
                    other.avatar_color_index,
                ))
            });
        let name = message
            .sender_name
            .clone()
            .filter(|n| !n.is_empty())
            .or_else(|| shared_agent.as_ref().map(|(_, n, _)| n.clone()))
            .or_else(|| project.map(|p| p.name.clone()))
            .unwrap_or_else(|| "Agent".to_string());
        let (seed, index) = shared_agent
            .as_ref()
            .map(|(s, _, i)| (s.clone(), *i))
            .or_else(|| project.map(|p| (p.id.clone(), p.avatar_color_index)))
            .unwrap_or_else(|| (project_id.to_string(), None));
        let image = crate::ui::avatars::notification_png(
            &seed,
            &name,
            index,
            crate::ui::theme::system_is_dark(),
        );
        (name, image)
    }

    fn notify_for_message(
        &mut self,
        chat_id: &str,
        message: &Message,
        was_streaming: bool,
        had_open_question: bool,
        is_new: bool,
    ) {
        let Some(chat) = self
            .chats
            .values()
            .flat_map(|v| v.iter())
            .find(|c| c.id == chat_id)
        else {
            return;
        };
        let project_id = chat.project_id.clone();
        let title = chat.title.clone();
        let is_system = chat.is_system;
        // Single-chat mode: a chat the UI cannot open does not notify either.
        if !self
            .visible_tabs(&project_id)
            .iter()
            .any(|c| c.id == chat_id)
        {
            return;
        }

        let sender = message.sender.as_str();
        let in_people = self
            .projects
            .iter()
            .any(|p| p.id == project_id && p.is_people());
        if message.role == Role::User {
            // Someone else wrote in a people chat (D72 + D49): "@sam: …".
            if is_new && in_people && !self.is_mine(message) {
                let handle = message
                    .sender_handle
                    .clone()
                    .unwrap_or_else(|| "someone".into());
                let person = self
                    .projects
                    .iter()
                    .find(|p| p.id == project_id)
                    .and_then(|p| p.people.iter().find(|x| x.handle == handle).cloned());
                let name = message
                    .sender_name
                    .clone()
                    .filter(|n| !n.is_empty())
                    .or_else(|| person.as_ref().map(|x| x.display_name()))
                    .unwrap_or_else(|| format!("@{handle}"));
                let url = message
                    .sender_avatar
                    .clone()
                    .filter(|a| a.contains("://"))
                    .or_else(|| person.as_ref().map(|x| x.avatar_url.clone()))
                    .unwrap_or_default();
                let image = self.person_notification_png(&url);
                self.notify_from(
                    project_id,
                    Some(chat_id),
                    NotifyKind::DirectMessage,
                    preview(message),
                    Some(name),
                    image,
                );
                return;
            }
            // A prompt another project injected is news; our own typed
            // messages and routine prompts are not (the routine's reply is).
            if is_new && (sender.starts_with("project:") || (is_system && sender != "user")) {
                self.notify(
                    project_id,
                    Some(chat_id),
                    NotifyKind::CrossProject,
                    preview(message),
                );
            }
            return;
        }
        if needs_input(message) && !had_open_question {
            let question = message
                .open_questions()
                .next_back()
                .and_then(|p| match p {
                    Part::Question { question, .. } => Some(question.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| preview(message));
            self.notify(project_id, Some(chat_id), NotifyKind::NeedsInput, question);
            return;
        }
        let finished = !message.is_streaming() && message.status != MessageStatus::Queued;
        if !finished || !(was_streaming || is_new) {
            return;
        }
        if message.role == Role::Assistant && was_streaming {
            // The avatar's short reply pulse, once per completed turn.
            self.avatar_reply_until
                .insert(project_id.clone(), now_ms() + 1000);
        }
        if message.role == Role::Project || sender.starts_with("project:") {
            let (sender, image) = self.agent_identity(&project_id, message);
            self.notify_from(
                project_id,
                Some(chat_id),
                NotifyKind::CrossProject,
                preview(message),
                Some(sender),
                image,
            );
            return;
        }
        if message.role != Role::Assistant {
            return;
        }
        let failed = message.status == MessageStatus::Error;
        if let Some(routine) = self.routine_behind(chat_id, message) {
            let detail = if failed {
                format!("{routine}: run failed")
            } else {
                let p = preview(message);
                if p.is_empty() {
                    routine
                } else {
                    format!("{routine}: {p}")
                }
            };
            self.notify(project_id, Some(chat_id), NotifyKind::RoutineRan, detail);
            return;
        }
        let detail = if failed {
            format!("{title}: turn failed")
        } else {
            let p = preview(message);
            if p.is_empty() {
                title
            } else {
                p
            }
        };
        let (sender, image) = self.agent_identity(&project_id, message);
        self.notify_from(
            project_id,
            Some(chat_id),
            NotifyKind::TurnFinished,
            detail,
            Some(sender),
            image,
        );
    }

    /// The routine whose prompt this assistant reply answers (F3: the prompt is
    /// a user-role message with `sender: routine:<id>` and `routineName`).
    fn routine_behind(&self, chat_id: &str, reply: &Message) -> Option<String> {
        let list = self.messages_of(chat_id);
        let prompt = match reply.in_reply_to.as_deref() {
            Some(id) => list.iter().find(|m| m.id == id),
            None => {
                let at = list
                    .iter()
                    .position(|m| m.id == reply.id)
                    .unwrap_or(list.len());
                list[..at].iter().rev().find(|m| m.role == Role::User)
            }
        }?;
        let id = prompt.sender.strip_prefix("routine:")?;
        Some(
            prompt
                .routine_name
                .clone()
                .unwrap_or_else(|| format!("Routine {id}")),
        )
    }

    /// F2 approvals call this when a gate card appears; no caller on this
    /// branch yet.
    #[allow(dead_code)]
    pub fn notify_approval_requested(
        &mut self,
        project_id: Id,
        chat_id: Option<&str>,
        what: String,
    ) {
        self.notify(project_id, chat_id, NotifyKind::ApprovalRequested, what);
    }

    fn notify_tick(&mut self, cx: &mut Context<Self>) {
        self.tick_calls(cx);
        if self.account.is_some() {
            // The explainer, once the OS has answered the start-up status query.
            let before = self.notify_prompt;
            self.maybe_prompt_notifications();
            if self.notify_prompt != before {
                cx.notify();
            }
        }
        // Keep Settings → Notifications truthful (System Settings can change it any time).
        notify::refresh_permission();
        let gates: HashMap<Id, Gate> = self
            .projects
            .iter()
            .map(|p| (p.id.clone(), self.notify_gate(&p.id, None)))
            .collect();
        self.notifier
            .tick(now_ms(), |id| gates.get(id).copied().unwrap_or_default());
        for click in notify::take_clicks() {
            if let Some(rest) = click.strip_prefix("call:") {
                match rest.split_once(':') {
                    Some(("accept", _)) => self.accept_incoming(cx),
                    Some(("decline", _)) => self.decline_incoming(cx),
                    _ => {}
                }
                continue;
            }
            self.open_chat_from_notification(click, cx);
        }
        self.sync_badge();
        // Selection paths that bypass select_tab/select_project still report.
        self.sync_focus();
    }

    /// A server channel message (relay `channel_message` push, from the homeserver's push
    /// rules — every member but the sender): a banner "<sender> in #channel" unless that
    /// channel is on screen in a focused window or the server is muted. The coalescer keys
    /// it under `server:<id>` so a busy server folds into "N updates in <server>".
    #[allow(clippy::too_many_arguments)]
    fn notify_channel_message(
        &mut self,
        server_id: &str,
        server_name: &str,
        channel_id: &str,
        channel_name: &str,
        room_id: &str,
        sender: &crate::model::Sender,
        text: Option<&str>,
    ) {
        let key = format!("server:{server_id}");
        let visible = self.window_active
            && self
                .selected_channel
                .as_ref()
                .is_some_and(|c| c.server_id == server_id && c.channel_id == channel_id);
        let gate = Gate {
            streaming: false,
            visible: visible || self.is_muted(&key),
        };
        let title = if sender.kind == "agent" || sender.handle.is_empty() {
            sender.name.clone()
        } else {
            format!("@{}", sender.handle)
        };
        // Same shape as a group banner: title = where ("Server › #channel"), body = "Sender: text".
        let (banner_title, body) = notify::channel_frame(server_name, channel_name, &title, text);
        let mut event = NotifyEvent::new(
            key,
            server_name.to_string(),
            NotifyKind::DirectMessage,
            body,
        );
        event.chat_id = Some(room_id.to_string());
        event.sender = Some(banner_title);
        event.image = if sender.kind == "agent" {
            crate::ui::avatars::notification_png(
                &sender.id,
                &sender.name,
                sender.avatar_color_index,
                crate::ui::theme::system_is_dark(),
            )
        } else if sender.avatar_url.is_empty() {
            None
        } else {
            self.person_notification_png(&sender.avatar_url)
        };
        self.notifier.offer(event, now_ms(), gate);
    }

    /// A click on a notification: bring the window up and show that chat — or, for a
    /// server channel banner (its id is the room id), that channel.
    pub fn open_chat_from_notification(&mut self, chat_id: Id, cx: &mut Context<Self>) {
        let channel = self.servers.iter().find_map(|s| {
            s.channels
                .iter()
                .find(|c| c.room_id.as_deref() == Some(chat_id.as_str()))
                .map(|c| (s.id.clone(), c.id.clone()))
        });
        if let Some((server_id, channel_id)) = channel {
            self.select_channel(server_id, channel_id, cx);
            cx.activate(true);
            return;
        }
        let project_id = self
            .chats
            .values()
            .flat_map(|v| v.iter())
            .find(|c| c.id == chat_id)
            .map(|c| c.project_id.clone());
        let Some(project_id) = project_id else {
            return;
        };
        self.select_project(project_id, cx);
        self.select_tab(chat_id, cx);
        cx.activate(true);
    }

    /// Dock badge = unread messages across chats (from `GET /me/unread`; cleared as they are read).
    pub fn sync_badge(&self) {
        let total = self
            .chats
            .values()
            .flat_map(|v| v.iter())
            .map(Chat::missed_count)
            .sum();
        notify::set_badge(total);
    }

    /// Signed in and macOS has not decided yet: show the explainer (once per
    /// launch; "Not now" snoozes it for a day, then it comes back because the
    /// system is still undecided). Allowed or denied in the system → nothing
    /// to show; Settings → Notifications says which and opens System Settings
    /// for a denial.
    fn maybe_prompt_notifications(&mut self) {
        if self.notify_prompt || self.notify_prompt_answered || !notify::needs_permission() {
            return;
        }
        match notify::permission() {
            notify::Permission::NotDetermined => {
                if notify::prompt_snoozed(&crate::sync::credentials::config_dir()) {
                    self.notify_prompt_answered = true;
                } else {
                    self.notify_prompt = true;
                }
            }
            // The OS has not answered yet; `notify_tick` asks again.
            notify::Permission::Unknown => {}
            notify::Permission::Denied | notify::Permission::Allowed => {
                self.notify_prompt_answered = true;
            }
        }
    }

    pub fn allow_notifications(&mut self, cx: &mut Context<Self>) {
        notify::snooze_prompt(&crate::sync::credentials::config_dir());
        notify::request_permission();
        self.notify_prompt = false;
        self.notify_prompt_answered = true;
        cx.notify();
    }

    pub fn dismiss_notify_prompt(&mut self, cx: &mut Context<Self>) {
        notify::permission_dismissed(&crate::sync::credentials::config_dir());
        self.notify_prompt = false;
        self.notify_prompt_answered = true;
        cx.notify();
    }

    /// Settings → Notifications: the system's answer, refreshed every tick so
    /// a change made in System Settings shows up without a relaunch.
    pub fn notification_permission(&self) -> notify::Permission {
        notify::permission()
    }

    pub fn open_notification_settings(&mut self) {
        notify::open_system_settings();
    }

    pub fn set_window_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.window_active != active {
            self.window_active = active;
            if active {
                self.became_active_at = now_ms();
            }
            self.sync_focus();
            if active {
                if let Some(room_id) = self
                    .selected_channel
                    .as_ref()
                    .and_then(|c| c.room_id.clone())
                {
                    self.command(ClientCommand::ChannelMarkRead { room_id });
                }
            }
            cx.notify();
        }
    }

    /// Tell the relay which chat is on screen in a focused window (API §7c:
    /// presence `focused_chat`), whenever that changes.
    fn sync_focus(&mut self) {
        // Channels (D88) are not relay rooms: presence frames carry no focused chat for them.
        let focused = if self.window_active && self.selected_channel.is_none() {
            self.active_chat_id()
        } else {
            None
        };
        if self.focus_sent.as_ref() == Some(&focused) {
            return;
        }
        self.focus_sent = Some(focused.clone());
        self.command(ClientCommand::FocusChat { chat_id: focused });
    }

    pub fn set_notifications_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.notifier.enabled = enabled;
        cx.notify();
    }

    /// Settings → Notifications "Read receipts": Presence `{seen}` on/off.
    pub fn set_read_receipts(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.read_receipts == enabled {
            return;
        }
        self.read_receipts = enabled;
        save_read_receipts(enabled);
        if !enabled {
            self.message_receipts.clear();
        }
        self.command(ClientCommand::SetReadReceipts {
            enabled,
            persist: true,
        });
        cx.notify();
    }

    /// An info toast: something that worked or a plain status. Never a failure — that is
    /// [`Self::push_failure`] (`error_ui_lint` keeps raw error strings out of here). The same
    /// text again while its toast stands restarts that toast instead of adding one.
    pub fn push_notice(&mut self, text: impl Into<String>) {
        let text = text.into();
        let now = self.prune_notices();
        if let Some(n) = self
            .notices
            .iter_mut()
            .find(|n| n.error.is_none() && n.text == text)
        {
            n.at = now;
            n.count += 1;
            return;
        }
        let id = self.next_notice_id(now);
        self.notices.push(Notice {
            id,
            text,
            at: now,
            error: None,
            details_open: false,
            held: false,
            held_since: 0,
            retry: None,
            count: 1,
        });
    }

    /// An error toast (the shared error element, 10 s, hover holds, Details + Copy, Try again
    /// re-issues `retry`). The surface that owns the action catches most failures first
    /// (sheets, pane pages, the composer); this is for the rest. The same failure again while
    /// its toast stands (same sentence) bumps the toast's count and restarts its clock — one
    /// note per failure, however often it repeats.
    pub fn push_failure(&mut self, error: UserError, retry: Option<Box<ClientCommand>>) {
        let now = self.prune_notices();
        if let Some(n) = self.notices.iter_mut().find(|n| n.same_failure(&error)) {
            n.at = now;
            n.count = n.count.saturating_add(1);
            n.error = Some(error);
            if retry.is_some() {
                n.retry = retry;
            }
            return;
        }
        let id = self.next_notice_id(now);
        self.notices.push(Notice {
            id,
            text: error.sentence(),
            at: now,
            error: Some(error),
            details_open: false,
            held: false,
            held_since: 0,
            retry,
            count: 1,
        });
    }

    /// Drops the expired toasts; returns `now`.
    fn prune_notices(&mut self) -> u64 {
        let now = now_ms();
        self.notices.retain(|n| n.live_at(now));
        now
    }

    /// A unique id for the next toast (`now`, or one past the newest id when two arrive in
    /// the same millisecond).
    fn next_notice_id(&self, now: u64) -> u64 {
        let last = self.notices.iter().map(|n| n.id).max().unwrap_or(0);
        now.max(last + 1)
    }

    /// Click on × (or an info toast) removes it (download status / errors must not sit forever).
    pub fn dismiss_notice(&mut self, id: u64) {
        self.notices.retain(|n| n.id != id);
    }

    /// Clears the whole slot (the "N more" line's ×).
    pub fn dismiss_all_notices(&mut self) {
        self.notices.clear();
    }

    /// The pointer on a toast holds it; leaving restarts its clock.
    pub fn hold_notice(&mut self, id: u64, held: bool) {
        let now = now_ms();
        if let Some(n) = self.notices.iter_mut().find(|n| n.id == id) {
            if held && !n.held {
                n.held_since = now;
            }
            n.held = held;
            if !held {
                n.at = now;
            }
        }
    }

    /// A surface with a form up takes its verbs' failures (drawn inline at its top); `None`
    /// releases them. Changing the claim drops a note the old surface held.
    pub fn set_failure_claim(&mut self, claim: Option<FailureClaim>) {
        if self.failure_claim != claim {
            self.failure_claim = claim;
            self.claimed_failure = None;
        }
    }

    pub fn clear_claimed_failure(&mut self) {
        self.claimed_failure = None;
    }

    pub fn toggle_claimed_failure_details(&mut self) {
        if let Some(slot) = self.claimed_failure.as_mut() {
            slot.toggle_details();
        }
    }

    pub fn toggle_sign_in_details(&mut self) {
        if let Some(p) = &mut self.pairing {
            p.error_details_open = !p.error_details_open;
        }
    }

    pub fn toggle_notice_details(&mut self, id: u64) {
        let now = now_ms();
        if let Some(n) = self.notices.iter_mut().find(|n| n.id == id) {
            n.details_open = !n.details_open;
            if !n.held {
                n.held_since = now;
            }
            n.held = true;
        }
    }

    /// Try again on an error toast: re-issue its command and drop the toast.
    pub fn retry_notice(&mut self, id: u64) {
        let retry = self
            .notices
            .iter()
            .find(|n| n.id == id)
            .and_then(|n| n.retry.clone());
        self.dismiss_notice(id);
        if let Some(cmd) = retry {
            self.command(*cmd);
        }
    }

    /// Every toast still on screen, oldest first.
    pub fn live_notices(&self) -> Vec<Notice> {
        let now = now_ms();
        self.notices
            .iter()
            .filter(|n| n.live_at(now))
            .cloned()
            .collect()
    }

    /// What the slot draws: the newest [`NOTICE_VISIBLE_MAX`] toasts (oldest first) and the
    /// number folded away above them.
    pub fn notice_stack(&self) -> NoticeStack {
        let live = self.live_notices();
        let folded = live.len().saturating_sub(NOTICE_VISIBLE_MAX);
        NoticeStack {
            visible: live[folded..].to_vec(),
            folded,
        }
    }

    /// The composer's note for `chat`, if a send / attachment / recording failed there.
    pub fn send_error(&self, chat: &Id) -> Option<&error_note::Slot> {
        self.send_errors.get(chat)
    }

    pub fn set_send_error(&mut self, chat: Id, error: UserError) {
        self.send_errors.insert(chat, error_note::Slot::new(error));
    }

    pub fn toggle_send_error_details(&mut self, chat: &Id) {
        if let Some(slot) = self.send_errors.get_mut(chat) {
            slot.toggle_details();
        }
    }

    /// Try again on the composer's note: re-send the newest failed message in that chat (the
    /// note clears either way; an attachment / recording failure has nothing to re-send).
    pub fn retry_send_error(&mut self, chat: Id, cx: &mut Context<Self>) {
        self.send_errors.remove(&chat);
        let failed = self
            .messages
            .get(&chat)
            .and_then(|list| list.iter().rev().find(|m| m.status == MessageStatus::Error))
            .map(|m| m.id.clone());
        if let Some(message_id) = failed {
            self.resend_failed(chat, message_id, cx);
        }
        cx.notify();
    }

    pub fn pair(&mut self, code: String, cx: &mut Context<Self>) {
        if let Some(p) = &mut self.pairing {
            p.busy = true;
            p.error = None;
        }
        self.command(ClientCommand::Pair { code });
        cx.notify();
    }

    /// "Sign in with GitHub" (and Retry): run the browser leg, then exchange
    /// the grant. The pill follows `pairing.flow`; a 60-s soft timeout offers
    /// Retry / Open in browser without closing a sheet that is merely slow.
    pub fn sign_in(&mut self, cx: &mut Context<Self>) {
        let Some(p) = &mut self.pairing else {
            return;
        };
        let Some(attempt) = p.flow.start(now_ms()) else {
            return;
        };
        let relay = p.relay_url.clone();
        cx.notify();
        self.arm_sign_in_timeout(attempt, cx);
        let this = cx.entity().downgrade();
        auth::begin(relay, device_name(), cx, move |event, cx| {
            let _ = this.update(cx, |state, cx| {
                state.on_sign_in_event(attempt, event);
                cx.notify();
            });
        });
    }

    fn on_sign_in_event(&mut self, attempt: u64, event: auth::Event) {
        let Some(p) = &mut self.pairing else {
            return;
        };
        match event {
            auth::Event::Opened { in_browser } => p.flow.opened(attempt, in_browser),
            auth::Event::Finished(Ok(auth::SignIn::Grant(grant_code))) => {
                if p.flow.grant(attempt) {
                    self.command(ClientCommand::SignIn { grant_code });
                }
            }
            auth::Event::Finished(Ok(auth::SignIn::PairCodeMode)) => {
                if attempt == p.flow.attempt {
                    p.mode = SignInMode::PairCode;
                    p.flow.cancelled(attempt);
                }
            }
            auth::Event::Finished(Ok(auth::SignIn::Cancelled)) => p.flow.cancelled(attempt),
            auth::Event::Finished(Err(err)) => {
                p.flow.failed(attempt, UserError::classify("sign in", &err))
            }
        }
    }

    /// `sign_in::SOFT_TIMEOUT_MS` after the click: Retry / Open in browser.
    fn arm_sign_in_timeout(&mut self, attempt: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(sign_in::SOFT_TIMEOUT_MS))
                .await;
            let _ = this.update(cx, |state, cx| {
                if let Some(p) = &mut state.pairing {
                    if p.flow.tick(attempt, now_ms()) {
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// For each silenced people chat with an end time: clear it if the clock has
    /// passed, else arm one timer that clears `silenced` and a silenced
    /// `push_refused` when it fires ([feedback app-01M61GZ5B66VCSKF40458KH3DP]).
    fn arm_silence_end_timers(&mut self, cx: &mut Context<Self>) {
        let now = now_ms() as i64;
        let mut changed = false;
        for p in &mut self.projects {
            if crate::sync::clear_silence_if_expired(p, &mut self.push_refused, now) {
                changed = true;
                self.silence_until_armed.remove(&p.id);
                self.silence_until_gen.remove(&p.id);
            }
        }
        let to_arm: Vec<(Id, i64)> = self
            .projects
            .iter()
            .filter(|p| p.silenced)
            .filter_map(|p| p.silenced_until.map(|u| (p.id.clone(), u)))
            .collect();
        let armed: HashSet<_> = to_arm.iter().map(|(id, _)| id.clone()).collect();
        self.silence_until_armed.retain(|id, _| armed.contains(id));
        self.silence_until_gen.retain(|id, _| armed.contains(id));
        for (chat_id, until) in to_arm {
            if self.silence_until_armed.get(&chat_id) == Some(&until) {
                continue;
            }
            self.silence_until_armed.insert(chat_id.clone(), until);
            let gen = self.silence_until_gen.entry(chat_id.clone()).or_insert(0);
            *gen = gen.wrapping_add(1);
            let gen = *gen;
            let wait_ms = until.saturating_sub(now).max(0) as u64;
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(wait_ms))
                    .await;
                let _ = this.update(cx, |state, cx| {
                    if state.silence_until_gen.get(&chat_id) != Some(&gen) {
                        return;
                    }
                    let Some(p) = state.projects.iter_mut().find(|p| p.id == chat_id) else {
                        return;
                    };
                    if crate::sync::clear_silence_if_expired(
                        p,
                        &mut state.push_refused,
                        now_ms() as i64,
                    ) {
                        state.silence_until_armed.remove(&chat_id);
                        state.silence_until_gen.remove(&chat_id);
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        if changed {
            cx.notify();
        }
    }

    /// Cancel on the pill: close the sheet (its block reports Cancelled) and
    /// go back to "Sign in with GitHub" at once.
    pub fn cancel_sign_in(&mut self, cx: &mut Context<Self>) {
        auth::cancel();
        if let Some(p) = &mut self.pairing {
            let attempt = p.flow.attempt;
            p.flow.cancelled(attempt);
        }
        cx.notify();
    }

    /// "Open in browser" on the pill: hand the GitHub page to the default
    /// browser; the relay's `arbos://auth/callback` comes back through
    /// LaunchServices (`sign_in_callback`).
    pub fn sign_in_in_browser(&mut self, cx: &mut Context<Self>) {
        let Some(p) = &mut self.pairing else {
            return;
        };
        let Some(attempt) = p.flow.continue_in_browser(now_ms()) else {
            return;
        };
        auth::cancel();
        let url = auth::start_url(&p.relay_url, &device_name());
        if let Err(err) = auth::open_in_browser(&url) {
            p.flow
                .failed(attempt, UserError::classify("open the browser", &err));
        } else {
            self.arm_sign_in_timeout(attempt, cx);
        }
        cx.notify();
    }

    /// `arbos://auth/callback?code=…` delivered by the platform (the browser
    /// leg). Ignored when no sign-in is in flight.
    pub fn sign_in_callback(&mut self, url: &str, cx: &mut Context<Self>) {
        let Some(p) = &mut self.pairing else {
            return;
        };
        let attempt = p.flow.attempt;
        if !p.flow.busy() {
            return;
        }
        match auth::parse_callback(url) {
            Ok(grant_code) => {
                if p.flow.grant(attempt) {
                    self.command(ClientCommand::SignIn { grant_code });
                }
            }
            Err(err) => p.flow.failed(attempt, UserError::classify("sign in", &err)),
        }
        cx.notify();
    }

    // ----- people chats (D72)

    /// `gh_<id>` of the signed-in user; the fixture signs in as `gh_1`.
    pub fn me_id(&self) -> String {
        self.account
            .as_ref()
            .map(|a| a.id.clone())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| {
                if self.source_name == "fixture" {
                    crate::sync::fixture::ME_ID.into()
                } else {
                    String::new()
                }
            })
    }

    pub fn me_handle(&self) -> String {
        self.account
            .as_ref()
            .map(|a| a.login.clone())
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| {
                if self.source_name == "fixture" {
                    crate::sync::fixture::ME_HANDLE.into()
                } else {
                    String::new()
                }
            })
    }

    /// The signed-in user's display name, else their handle.
    pub fn me_name(&self) -> String {
        self.account
            .as_ref()
            .map(|a| a.name.trim().to_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| self.me_handle())
    }

    /// Projects of mine (not people chats, not shared with me) — the add-agent picker.
    pub fn my_projects(&self) -> impl Iterator<Item = &Project> {
        self.projects.iter().filter(|p| !p.is_people() && !p.shared)
    }

    pub fn people_spaces(&self) -> impl Iterator<Item = &Project> {
        self.projects.iter().filter(|p| p.is_people())
    }

    /// Is this message mine? (`sender: user:<my id>` or the plain `user` of a project chat.)
    pub fn is_mine(&self, message: &Message) -> bool {
        let sender = message.sender.as_str();
        sender == "user" || sender == format!("user:{}", self.me_id())
    }

    pub fn open_new_chat(&mut self, cx: &mut Context<Self>) {
        self.new_chat_open = true;
        self.new_project_open = false;
        self.new_group_open = false;
        self.new_group_picked.clear();
        self.new_group_agents.clear();
        self.invite_target = None;
        self.message_grant_target = None;
        self.add_members_target = None;
        self.add_members_server = None;
        self.add_members_secret = None;
        self.people_query.clear();
        self.people_error = None;
        // First frame = the known directory; the refetch behind it only repaints a change.
        let (results, busy) = sheet_first_paint(&self.people_directory);
        self.people_results = results;
        self.people_busy = busy;
        self.command(ClientCommand::SearchPeople {
            query: String::new(),
        });
        cx.notify();
    }

    /// Fetch the directory ahead of the first "+" (once the account is known).
    fn prefetch_directory(&mut self) {
        if self.people_directory.is_empty() && !self.new_chat_open {
            self.command(ClientCommand::SearchPeople {
                query: String::new(),
            });
        }
    }

    /// Project header "Invite": the same sheet, but a pick adds a member.
    pub fn open_invite(&mut self, project_id: Id, cx: &mut Context<Self>) {
        self.open_new_chat(cx);
        self.invite_target = Some(project_id);
        cx.notify();
    }

    /// "Give a person access…": the same sheet; a pick lets that person's agents message
    /// this agent (`PUT /projects/{id}/message-access`). The agent pane's "Who may message"
    /// card that opened it left on 2026-10-25 ([feedback
    /// agent-details-remove-sections-mac-2026-10-25]); the list and its calls stay for the
    /// next way in.
    #[allow(dead_code)]
    pub fn open_message_grant_picker(&mut self, project_id: Id, cx: &mut Context<Self>) {
        self.open_new_chat(cx);
        self.message_grant_target = Some(project_id);
        cx.notify();
    }

    #[allow(dead_code)]
    pub fn set_message_grant(
        &mut self,
        project_id: Id,
        grantee: String,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == project_id) {
            if let Some(g) = p.message_grants.iter_mut().find(|g| g.grantee == grantee) {
                g.enabled = enabled;
            }
        }
        self.command(ClientCommand::SetMessageGrant {
            project_id,
            grantee,
            enabled,
        });
        cx.notify();
    }

    #[allow(dead_code)]
    pub fn remove_message_grant(
        &mut self,
        project_id: Id,
        grantee: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == project_id) {
            p.message_grants.retain(|g| g.grantee != grantee);
        }
        self.command(ClientCommand::RemoveMessageGrant {
            project_id,
            grantee,
        });
        cx.notify();
    }

    pub fn close_new_chat(&mut self, cx: &mut Context<Self>) {
        self.new_chat_open = false;
        self.new_group_open = false;
        self.new_group_picked.clear();
        self.new_group_agents.clear();
        self.awaiting_dm = None;
        self.awaiting_group = None;
        self.invite_target = None;
        self.message_grant_target = None;
        self.add_members_target = None;
        self.add_members_server = None;
        self.add_members_secret = None;
        cx.notify();
    }

    /// Secret page › People → "Give a person access…": the same picker, people only, whose
    /// Add puts the picks on the secret's People list (spec §7.7).
    pub fn open_secret_people_picker(&mut self, secret_id: Id, cx: &mut Context<Self>) {
        if !self.secrets.secrets.iter().any(|s| s.id == secret_id) {
            return;
        }
        self.open_new_chat(cx);
        self.new_group_open = true;
        self.add_members_secret = Some(secret_id);
        cx.notify();
    }

    /// Server pane › Members → "Add members" (D88, the main way in since 2026-10-22): the same
    /// picker as groups, people only, whose Add puts the picks into the server.
    pub fn open_add_server_members(&mut self, server_id: Id, cx: &mut Context<Self>) {
        if !self.server(&server_id).is_some_and(|s| s.can_admin()) {
            return;
        }
        self.open_new_chat(cx);
        self.new_group_open = true;
        self.add_members_server = Some(server_id);
        cx.notify();
    }

    /// Who the Add members picker adds to (a group, a server, or a secret's People list),
    /// with who is already in.
    pub fn add_members_target_info(&self) -> Option<AddTarget> {
        if let Some(secret) = self
            .add_members_secret
            .as_deref()
            .and_then(|id| self.secrets.secrets.iter().find(|s| s.id == id))
        {
            let mut handles: Vec<String> =
                secret.directors.iter().map(|d| d.handle.clone()).collect();
            handles.push(self.me_handle());
            return Some(AddTarget {
                name: secret.env.clone(),
                handles,
                agents: Vec::new(),
                is_server: true,
                is_secret: true,
            });
        }
        if let Some(server) = self
            .add_members_server
            .as_deref()
            .and_then(|id| self.server(id))
        {
            let mut handles: Vec<String> =
                server.members.iter().map(|m| m.handle.clone()).collect();
            handles.push(self.me_handle());
            return Some(AddTarget {
                name: server.name.clone(),
                handles,
                agents: Vec::new(),
                is_server: true,
                is_secret: false,
            });
        }
        let group = self.add_members_group()?;
        let mut handles: Vec<String> = group.people.iter().map(|p| p.handle.clone()).collect();
        handles.push(self.me_handle());
        Some(AddTarget {
            name: group.name.clone(),
            handles,
            agents: group.agents.iter().map(|a| a.project_id.clone()).collect(),
            is_server: false,
            is_secret: false,
        })
    }

    /// Group pane → "Add members": the New group picker, titled for this
    /// group, whose Add puts the picks into it (not into a new group).
    pub fn open_add_members(&mut self, chat_id: Id, cx: &mut Context<Self>) {
        if !self.project(&chat_id).is_some_and(|p| p.is_people()) {
            return;
        }
        self.open_new_chat(cx);
        self.new_group_open = true;
        self.add_members_target = Some(chat_id);
        cx.notify();
    }

    /// The group the picker adds to, when it was opened from a group pane.
    pub fn add_members_group(&self) -> Option<&Project> {
        self.add_members_target
            .as_deref()
            .and_then(|id| self.project(id))
    }

    /// "Add" in the Add members picker: one relay call per new person
    /// (`POST /chats/{id}/members`) and per new agent (`POST /chats/{id}/agents`),
    /// all on the group the picker was opened for. People already in the group
    /// are skipped (the rows were disabled anyway). The pane shows them at once;
    /// the relay's next list confirms. Returns whether the calls were sent (the
    /// sheet closes then; an empty pick keeps it open with a line why).
    pub fn add_members_to_group(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some(secret_id) = self.add_members_secret.clone() {
            return self.add_people_to_secret(secret_id, cx);
        }
        if let Some(server_id) = self.add_members_server.clone() {
            return self.add_members_to_server(server_id, cx);
        }
        let Some(space) = self.add_members_group().cloned() else {
            return false;
        };
        let me_handle = self.me_handle();
        let plan = add_members_plan(
            &space,
            &me_handle,
            &self.new_group_picked,
            &self.new_group_agents,
        );
        if plan.handles.is_empty() && plan.agent_ids.is_empty() {
            self.people_error = Some(refused(
                "add members",
                "Pick someone who is not in the group yet",
            ));
            cx.notify();
            return false;
        }
        let picked: Vec<Person> = self
            .new_group_picked
            .iter()
            .filter(|p| plan.handles.contains(&p.handle))
            .cloned()
            .collect();
        // Named the way the relay names them: a person by name, else @handle; an agent by
        // its project name.
        let mut names: Vec<String> = picked
            .iter()
            .map(|p| match p.name.as_deref().map(str::trim) {
                Some(name) if !name.is_empty() => name.to_owned(),
                _ => format!("@{}", p.handle),
            })
            .collect();
        names.extend(plan.agent_ids.iter().filter_map(|id| {
            self.projects
                .iter()
                .find(|p| &p.id == id)
                .map(|p| p.name.clone())
        }));
        let mut subjects: Vec<NoticeSubject> = picked
            .iter()
            .map(|p| NoticeSubject {
                kind: "user".into(),
                id: p.id.clone(),
            })
            .collect();
        subjects.extend(plan.agent_ids.iter().map(|id| NoticeSubject {
            kind: "agent".into(),
            id: id.clone(),
        }));
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == plan.chat_id) {
            p.people.extend(picked);
        }
        self.show_membership_notice(&plan.chat_id, &names, subjects, cx);
        self.command(ClientCommand::AddMembers {
            chat_id: plan.chat_id,
            handles: plan.handles,
            agent_ids: plan.agent_ids,
        });
        cx.notify();
        true
    }

    /// "Add" in the picker opened from a Secret page: `POST /secrets/{id}/people` per new
    /// person; people already on the list are skipped. The card shows them at once (rows on,
    /// no note yet); the relay's next list confirms.
    fn add_people_to_secret(&mut self, secret_id: Id, cx: &mut Context<Self>) -> bool {
        let Some(secret) = self.secrets.secrets.iter().find(|s| s.id == secret_id) else {
            return false;
        };
        let me = self.me_handle();
        let new: Vec<Person> = self
            .new_group_picked
            .iter()
            .filter(|p| {
                !p.handle.eq_ignore_ascii_case(&me)
                    && !secret
                        .directors
                        .iter()
                        .any(|d| d.user_id == p.id || d.handle.eq_ignore_ascii_case(&p.handle))
            })
            .cloned()
            .collect();
        if new.is_empty() {
            self.people_error = Some(refused(
                "add them",
                "Pick someone who is not on the list yet",
            ));
            cx.notify();
            return false;
        }
        let handles: Vec<String> = new.iter().map(|p| p.handle.clone()).collect();
        if let Some(s) = self.secrets.secrets.iter_mut().find(|s| s.id == secret_id) {
            s.directors.extend(new.into_iter().map(|p| SecretDirector {
                user_id: p.id,
                handle: p.handle,
                name: p.name,
                avatar_url: Some(p.avatar_url),
                note: String::new(),
                created_at: now_ms() as i64,
                enabled: true,
                disabled_at: None,
                scope: SecretScope::default(),
                last_asked_at: None,
            }));
        }
        self.secret_people_add(secret_id, handles, cx);
        true
    }

    /// "Add" in the picker opened from a server pane: `POST /servers/{id}/members` per new
    /// person; people already in the server are skipped. The pane shows them at once.
    fn add_members_to_server(&mut self, server_id: Id, cx: &mut Context<Self>) -> bool {
        let Some(server) = self.server(&server_id).cloned() else {
            return false;
        };
        let new: Vec<Person> = self
            .new_group_picked
            .iter()
            .filter(|p| {
                !server
                    .members
                    .iter()
                    .any(|m| m.id == p.id || m.handle.eq_ignore_ascii_case(&p.handle))
            })
            .cloned()
            .collect();
        if new.is_empty() {
            self.people_error = Some(refused(
                "add members",
                "Pick someone who is not in the server yet",
            ));
            cx.notify();
            return false;
        }
        let user_ids: Vec<Id> = new.iter().map(|p| p.id.clone()).collect();
        if let Some(s) = self.servers.iter_mut().find(|s| s.id == server_id) {
            s.members.extend(new.into_iter().map(|mut p| {
                p.role = "member".into();
                p
            }));
            s.member_count = s.members.len() as u64;
        }
        self.command(ClientCommand::ServerAddMembers {
            server_id,
            user_ids,
        });
        cx.notify();
        true
    }

    /// New Message → New group: multi-select people and optional agents.
    pub fn open_new_group(&mut self, cx: &mut Context<Self>) {
        self.new_group_open = true;
        self.new_project_open = false;
        self.new_group_picked.clear();
        self.new_group_agents.clear();
        self.people_error = None;
        self.people_query.clear();
        self.search_people(String::new(), cx);
        cx.notify();
    }

    /// Leave New group and return to the New Message sheet.
    pub fn close_new_group(&mut self, cx: &mut Context<Self>) {
        self.new_group_open = false;
        self.new_group_picked.clear();
        self.new_group_agents.clear();
        self.people_error = None;
        self.people_query.clear();
        self.search_people(String::new(), cx);
        cx.notify();
    }

    pub fn toggle_group_person(&mut self, person: Person, cx: &mut Context<Self>) {
        // Add members: someone already in the group is shown as added, not pickable.
        let me_handle = self.me_handle();
        if self
            .add_members_group()
            .is_some_and(|g| g.has_person(&me_handle, &person))
        {
            return;
        }
        if let Some(i) = self
            .new_group_picked
            .iter()
            .position(|p| p.id == person.id || p.handle == person.handle)
        {
            self.new_group_picked.remove(i);
        } else {
            self.new_group_picked.push(person);
        }
        self.people_error = None;
        cx.notify();
    }

    pub fn toggle_group_agent(&mut self, project_id: Id, cx: &mut Context<Self>) {
        if self
            .add_members_group()
            .is_some_and(|g| g.has_agent(&project_id))
        {
            return;
        }
        if let Some(i) = self
            .new_group_agents
            .iter()
            .position(|id| id == &project_id)
        {
            self.new_group_agents.remove(i);
        } else {
            self.new_group_agents.push(project_id);
        }
        self.people_error = None;
        cx.notify();
    }

    /// Create the people group (at least one other person), then add any agents.
    /// Create the group: people, agents, or both ("People and agents in one
    /// chat"); the relay answers with the chat and `PeopleGroupCreated` opens it.
    pub fn create_people_group(&mut self, cx: &mut Context<Self>) {
        let handles: Vec<String> = self
            .new_group_picked
            .iter()
            .map(|p| p.handle.clone())
            .collect();
        let agent_ids = self.new_group_agents.clone();
        if handles.is_empty() && agent_ids.is_empty() {
            self.people_error = Some(refused(
                "create the group",
                "Pick at least one person or agent",
            ));
            cx.notify();
            return;
        }
        self.people_busy = true;
        self.people_error = None;
        self.awaiting_group = Some(handles.clone());
        let (title, leaf, picture) = match self.new_group_draft.take() {
            Some(d) => (Some(d.title), d.leaf, d.picture),
            None => (None, None, None),
        };
        self.command(ClientCommand::CreatePeopleGroup {
            handles,
            agent_ids,
            title,
            leaf,
            picture,
        });
        cx.notify();
    }

    /// The group the sheet was waiting for exists: close the picker, open it.
    fn group_created(&mut self, chat_id: Id, cx: &mut Context<Self>) {
        self.awaiting_group = None;
        self.people_busy = false;
        self.people_error = None;
        self.new_chat_open = false;
        self.new_group_open = false;
        self.new_group_picked.clear();
        self.new_group_agents.clear();
        self.select_project(chat_id, cx);
        cx.notify();
    }

    /// The sheet's pick: invite into the target project or start a DM.
    pub fn pick_person(&mut self, handle: String, cx: &mut Context<Self>) {
        if self.new_group_open {
            if let Some(person) = self
                .people_results
                .iter()
                .find(|p| p.handle == handle)
                .cloned()
            {
                self.toggle_group_person(person, cx);
            }
            return;
        }
        if let Some(project_id) = self.message_grant_target.take() {
            self.new_chat_open = false;
            let handle = handle.trim().trim_start_matches('@').to_string();
            if !handle.is_empty() {
                self.command(ClientCommand::SetMessageGrant {
                    project_id,
                    grantee: format!("@{handle}"),
                    enabled: true,
                });
            }
            cx.notify();
            return;
        }
        match self.invite_target.clone() {
            Some(project_id) => {
                self.new_chat_open = false;
                self.invite_target = None;
                self.invite_to_project(project_id, handle, cx);
            }
            None => self.start_dm(handle, cx),
        }
    }

    pub fn search_people(&mut self, query: String, cx: &mut Context<Self>) {
        self.people_query = query.clone();
        self.people_error = None;
        // The known directory answers at once through the shared matcher
        // (display name and @handle — the relay's own search is a handle-prefix
        // lookup, which is why "aff" showed only @aff, [feedback
        // shared-search-matcher-mac-2026-10-26]); the relay's reply adds the
        // people the directory page did not carry.
        let local = people_sheet_results(&self.people_directory, &self.people_results, &query);
        // An empty query lists everyone on Arbos (the sheet's "Arbos users").
        self.people_busy = local.is_empty();
        self.people_results = local;
        self.command(ClientCommand::SearchPeople { query });
        cx.notify();
    }

    pub fn start_dm(&mut self, handle: String, cx: &mut Context<Self>) {
        let handle = handle.trim().trim_start_matches('@').to_string();
        if handle.is_empty() {
            return;
        }
        let existing = self
            .people_spaces()
            .find(|p| p.person().map(|x| x.handle == handle).unwrap_or(false))
            .map(|p| p.id.clone());
        if let Some(existing) = existing {
            self.new_chat_open = false;
            self.select_project(existing, cx);
            return;
        }
        self.open_draft_dm(handle, cx);
        cx.notify();
    }

    /// The handle `start_dm` is waiting on the relay for (tests).
    #[cfg(test)]
    pub(crate) fn awaiting_dm_handle(&self) -> Option<String> {
        self.awaiting_dm.clone()
    }

    /// Composer keystroke in a people chat: at most one presence ping per 3 s.
    pub fn typed_in(&mut self, chat_id: &Id) {
        self.last_input_at = now_ms();
        let Some(space) = self.project_of_chat(chat_id) else {
            return;
        };
        if !space.is_people() {
            return;
        }
        let now = now_ms();
        let last = self.typing_sent_at.get(chat_id).copied().unwrap_or(0);
        if now.saturating_sub(last) > 3_000 {
            self.typing_sent_at.insert(chat_id.clone(), now);
            self.command(ClientCommand::SetTyping {
                chat_id: chat_id.clone(),
                active: true,
            });
        }
    }

    /// Handles typing in `chat_id` right now, the most recently seen first.
    pub fn typing_in(&self, chat_id: &str) -> Vec<String> {
        let now = now_ms();
        let mut live: Vec<&(String, u64)> = self
            .typing
            .get(chat_id)
            .map(|v| v.iter().filter(|(_, until)| *until > now).collect())
            .unwrap_or_default();
        live.sort_by_key(|(_, until)| std::cmp::Reverse(*until));
        live.into_iter().map(|(handle, _)| handle.clone()).collect()
    }

    pub fn project_of_chat(&self, chat_id: &str) -> Option<&Project> {
        let pid = self
            .chats
            .values()
            .chain(self.job_tabs.values())
            .flat_map(|v| v.iter())
            .find(|c| c.id == chat_id)
            .map(|c| c.project_id.clone())?;
        self.projects.iter().find(|p| p.id == pid)
    }

    /// One of my agents into a people chat, the way the Add members picker
    /// does it (optimistic "<me> added <agent>" line, then the batch
    /// `AddMembers` call). Only the `ARBOS_RIG_ADD_AGENT` recording hook
    /// calls it since the "Add agent" rows went ([feedback
    /// add-your-agent-row-removal-mac-2026-10-27]); the picker goes through
    /// [`Self::add_members_to_group`].
    pub fn add_agent(&mut self, chat_id: Id, project_id: Id, cx: &mut Context<Self>) {
        let names: Vec<String> = self
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|p| vec![p.name.clone()])
            .unwrap_or_default();
        let subjects = vec![NoticeSubject {
            kind: "agent".into(),
            id: project_id.clone(),
        }];
        self.show_membership_notice(&chat_id, &names, subjects, cx);
        self.command(ClientCommand::AddMembers {
            chat_id,
            handles: Vec::new(),
            agent_ids: vec![project_id],
        });
        cx.notify();
    }

    /// The adding device's own "<me> added <names>" line, on screen the moment Add is pressed
    /// (ARB-393). The relay writes the row of record; [`Self::fold_optimistic_notices`] swaps
    /// this copy out when that row lands.
    fn show_membership_notice(
        &mut self,
        chat_id: &str,
        names: &[String],
        subjects: Vec<NoticeSubject>,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = added_line(&self.me_name(), names) else {
            return;
        };
        let now = now_ms();
        let me_id = self.me_id();
        let facts = (!me_id.is_empty() && !subjects.is_empty()).then(|| MembershipNotice {
            kind: "added".into(),
            actor: me_id,
            subjects,
        });
        let notice = OptimisticNotice::new(text, now, facts);
        if trace_rows() {
            eprintln!(
                "rows: optimistic-notice chat={chat_id} id={} text={:?} at={now}",
                notice.id, notice.text
            );
        }
        let list = self.messages.entry(chat_id.to_owned()).or_default();
        list.push(notice.message(chat_id));
        self.optimistic_notices
            .entry(chat_id.to_owned())
            .or_default()
            .push(notice);
        cx.emit(AppEvent::TranscriptChanged {
            chat_id: chat_id.to_owned(),
        });
    }

    /// A fresh relay snapshot replaces the chat's list: keep the local notices it has not
    /// confirmed yet, drop the ones it has (or that are past their TTL).
    fn fold_optimistic_notices(&mut self, chat_id: &str, messages: &mut Vec<Message>) {
        let Some(notices) = self.optimistic_notices.get_mut(chat_id) else {
            return;
        };
        let before = notices.len();
        let confirmed = reconcile_optimistic_notices(messages, notices, now_ms());
        if trace_rows() && (confirmed > 0 || notices.len() != before) {
            eprintln!(
                "rows: notice-reconciled chat={chat_id} confirmed={confirmed} left={} at={}",
                notices.len(),
                now_ms()
            );
        }
        if notices.is_empty() {
            self.optimistic_notices.remove(chat_id);
        }
    }

    /// A people-chat action failed: nothing was added, so no local notice may claim it was.
    fn withdraw_optimistic_notices(&mut self, cx: &mut Context<Self>) {
        let chats: Vec<Id> = self.optimistic_notices.drain().map(|(id, _)| id).collect();
        for chat_id in chats {
            if let Some(list) = self.messages.get_mut(&chat_id) {
                list.retain(|m| !m.id.starts_with(OptimisticNotice::ID_PREFIX));
            }
            cx.emit(AppEvent::TranscriptChanged { chat_id });
        }
    }

    pub fn remove_agent(&mut self, chat_id: Id, project_id: Id, cx: &mut Context<Self>) {
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == chat_id) {
            p.agents.retain(|a| a.project_id != project_id);
        }
        self.command(ClientCommand::RemoveAgent {
            chat_id,
            project_id,
        });
        cx.notify();
    }

    pub fn remove_member(&mut self, chat_id: Id, user_id: String, cx: &mut Context<Self>) {
        // The pane shows the result at once; the relay's list confirms it.
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == chat_id) {
            p.people.retain(|x| x.id != user_id);
        }
        self.command(ClientCommand::RemoveMember { chat_id, user_id });
        cx.notify();
    }

    /// Group owner: make a member an admin, or an admin a member again. The pane shows
    /// the new role at once; the relay's list confirms it.
    pub fn set_people_member_role(
        &mut self,
        chat_id: Id,
        user_id: String,
        role: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(m) = self
            .projects
            .iter_mut()
            .find(|p| p.id == chat_id)
            .and_then(|p| p.people.iter_mut().find(|m| m.id == user_id))
            .filter(|m| m.role != "owner")
        {
            m.role = role.to_owned();
        }
        self.command(ClientCommand::SetMemberRole {
            chat_id,
            user_id,
            role: role.to_owned(),
        });
        cx.notify();
    }

    pub fn leave_chat(&mut self, chat_id: Id, cx: &mut Context<Self>) {
        if self.selected_project.as_deref() == Some(chat_id.as_str()) {
            self.selected_project = None;
        }
        self.command(ClientCommand::LeaveChat { chat_id });
        cx.notify();
    }

    /// "Delete group" as a member: leave, and the row goes at once (the relay
    /// drops me from the chat; it stays for everyone else).
    pub fn leave_group(&mut self, chat_id: Id, cx: &mut Context<Self>) {
        if !self.project(&chat_id).is_some_and(|p| p.is_people_group()) {
            return;
        }
        if self.selected_project.as_deref() == Some(chat_id.as_str()) {
            self.selected_project = None;
        }
        self.messages.remove(&chat_id);
        self.search_index.remove_chat(&chat_id);
        self.chat_ui.remove(&chat_id);
        self.chats.remove(&chat_id);
        self.archived_chats.remove(&chat_id);
        self.unpin_if_pinned(&chat_id);
        self.projects.retain(|p| p.id != chat_id);
        self.muted.remove(&chat_id);
        self.command(ClientCommand::LeaveChat { chat_id });
        self.select_next_if_none(cx);
        cx.notify();
    }

    /// The centre is always a chat (three-pane rule): when the open one goes,
    /// the first remaining row takes its place.
    fn select_next_if_none(&mut self, cx: &mut Context<Self>) {
        if self.selected_project.is_some() {
            return;
        }
        if let Some(id) = self.projects.first().map(|p| p.id.clone()) {
            self.select_project(id, cx);
        }
    }

    pub fn block_user(&mut self, handle: String, cx: &mut Context<Self>) {
        self.command(ClientCommand::BlockUser { handle });
        cx.notify();
    }

    /// Is this person on my block list? Matched the way the relay keys blocks — by user
    /// id — with the handle as the fallback for rows that only carry a handle.
    pub fn is_blocked(&self, person: &Person) -> bool {
        blocked_entry(&self.blocked, person).is_some()
    }

    /// The senders whose rows `space` hides for me ([feedback blocked-state-mac-2026-10-26],
    /// Jacob 2026-10-26: a group block hides the blocked person's messages from the blocker
    /// alone): the relay's `blockedSenders` hint, plus every member on my block list (so a
    /// block taken a moment ago hides at once, before the chat list refreshes). `user:<id>`.
    pub fn hidden_senders_in(&self, space: &Project) -> HashSet<String> {
        let mut out: HashSet<String> = space.blocked_senders.iter().cloned().collect();
        for p in space.people.iter().filter(|p| self.is_blocked(p)) {
            if !p.id.is_empty() {
                out.insert(format!("user:{}", p.id));
            }
        }
        out
    }

    /// Is this message hidden for me — from someone I blocked, in a people group?
    pub fn hides_message(&self, chat_id: &str, m: &Message) -> bool {
        m.role == Role::User
            && m.sender.starts_with("user:")
            && self
                .project_of_chat(chat_id)
                .filter(|p| p.is_people_group())
                .is_some_and(|p| self.hidden_senders_in(p).contains(&m.sender))
    }

    /// `DELETE /blocks/{user_id}` for the block-list entry that matches `person`; a no-op
    /// when the relay does not list them (the row already reads Block user).
    pub fn unblock_user(&mut self, person: &Person, cx: &mut Context<Self>) {
        let Some(entry) = blocked_entry(&self.blocked, person) else {
            return;
        };
        let user_id = if entry.id.is_empty() {
            person.id.clone()
        } else {
            entry.id.clone()
        };
        if user_id.is_empty() {
            return;
        }
        let handle = if entry.handle.is_empty() {
            person.handle.clone()
        } else {
            entry.handle.clone()
        };
        self.command(ClientCommand::UnblockUser { user_id, handle });
        cx.notify();
    }

    pub fn invite_to_project(&mut self, project_id: Id, handle: String, cx: &mut Context<Self>) {
        let handle = handle.trim().trim_start_matches('@').to_string();
        if handle.is_empty() {
            return;
        }
        self.command(ClientCommand::InviteToProject { project_id, handle });
        cx.notify();
    }

    pub fn remove_project_member(
        &mut self,
        project_id: Id,
        user_id: String,
        cx: &mut Context<Self>,
    ) {
        self.command(ClientCommand::RemoveProjectMember {
            project_id,
            user_id,
        });
        cx.notify();
    }

    /// Decoded avatar for a URL, requesting it once when unknown.
    pub fn avatar(&self, url: &str) -> Option<Arc<RenderImage>> {
        self.avatars.get(url).cloned().flatten()
    }

    /// The bar colour for a person's bubbles: their picture's dominant colour
    /// once it has loaded, else the midpoint of their D79 disc gradient.
    pub fn sender_accent(&self, person: &Person, theme: &crate::ui::theme::Theme) -> gpui::Hsla {
        match self.accents.get(&person.avatar_url) {
            Some(accent) => crate::ui::avatars::accent_color(*accent, theme),
            None => crate::ui::avatars::accent_fallback(
                crate::ui::avatars::person_seed(person),
                None,
                theme,
            ),
        }
    }

    pub(crate) fn want_avatar(&mut self, url: String) {
        if url.is_empty() || self.avatars.contains_key(&url) {
            return;
        }
        self.avatars.insert(url.clone(), None);
        self.command(ClientCommand::FetchAvatar { url });
    }

    pub fn sign_out(&mut self, cx: &mut Context<Self>) {
        self.command(ClientCommand::SignOut);
        self.account = None;
        self.projects.clear();
        self.chats.clear();
        self.selected_project = None;
        self.people_directory.clear();
        self.people_results.clear();
        cx.notify();
    }

    pub fn create_project(
        &mut self,
        name: String,
        avatar_color_index: Option<u8>,
        leaf: Option<crate::ui::avatars::LeafPick>,
        picture: Option<crate::sync::DraftPicture>,
        cx: &mut Context<Self>,
    ) {
        self.new_project_open = false;
        self.awaiting_new_project = true;
        self.command(ClientCommand::CreateProject {
            name,
            avatar_color_index,
            leaf,
            picture,
            source_project_id: None,
            description: None,
            color: None,
        });
        self.push_notice("Creating project…");
        cx.notify();
    }

    /// Connect an outside agent (#2141): the relay makes the project and mints the key; the
    /// reveal sheet opens on [`SyncEvent::OutsideAgentKey`].
    pub fn connect_agent(
        &mut self,
        name: String,
        transport: &str,
        avatar_color_index: Option<u8>,
        cx: &mut Context<Self>,
    ) {
        self.connect_open = false;
        self.awaiting_new_project = true;
        self.command(ClientCommand::ConnectAgent {
            name,
            transport: transport.to_owned(),
            avatar_color_index,
        });
        self.push_notice("Connecting…");
        cx.notify();
    }

    /// While the reveal sheet is up, ask the relay every 3 s whether the agent has checked in.
    fn poll_connect_status(&mut self, cx: &mut Context<Self>) {
        let Some(project_id) = self.connect_reveal.as_ref().map(|r| r.project_id.clone()) else {
            return;
        };
        self.command(ClientCommand::OutsideAgentStatus {
            project_id: project_id.clone(),
        });
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(3))
                .await;
            let go = this.update(cx, |state, _| match state.connect_reveal.as_ref() {
                Some(r) if r.project_id == project_id => {
                    state.command(ClientCommand::OutsideAgentStatus {
                        project_id: project_id.clone(),
                    });
                    true
                }
                _ => false,
            });
            if !matches!(go, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    /// Done / × on the reveal sheet: the key is gone from the screen; open the agent's chat.
    pub fn close_connect_reveal(&mut self, cx: &mut Context<Self>) {
        if let Some(r) = self.connect_reveal.take() {
            if !r.rotated {
                if let Some(p) = self.projects.iter().find(|p| p.id == r.project_id) {
                    let id = p.id.clone();
                    self.select_project(id, cx);
                }
            }
        }
        cx.notify();
    }

    /// Details pane → Key → Rotate key…: a fresh key, the old one stops at once.
    pub fn rotate_outside_key(&mut self, project_id: Id, cx: &mut Context<Self>) {
        self.command(ClientCommand::OutsideAgentRotate { project_id });
        cx.notify();
    }

    /// Details pane → Key → Revoke access.
    pub fn revoke_outside_agent(&mut self, project_id: Id, cx: &mut Context<Self>) {
        self.command(ClientCommand::OutsideAgentRevoke { project_id });
        cx.notify();
    }

    /// Details pane → Can → a switch: one scope on or off (`PATCH /outside-agents/{id}`). The
    /// row flips at once; the relay's answer confirms the effective set.
    pub fn set_outside_scope(
        &mut self,
        project_id: Id,
        scope: &str,
        on: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == project_id) {
            if let Some(o) = p.outside.as_mut() {
                o.scopes.insert(scope.to_owned(), on);
            }
        }
        self.command(ClientCommand::OutsideAgentScope {
            project_id,
            scope: scope.to_owned(),
            on,
        });
        cx.notify();
    }

    /// Details pane: refresh the connection line when the pane opens on an outside agent.
    pub fn refresh_outside_status(&mut self, project_id: Id) {
        self.command(ClientCommand::OutsideAgentStatus { project_id });
    }

    /// The Wake URL form for this agent; a form left over from another agent is replaced.
    fn wake_form_mut(&mut self, project_id: &Id) -> &mut WakeForm {
        if self
            .wake_form
            .as_ref()
            .is_none_or(|f| &f.project_id != project_id)
        {
            self.wake_form = Some(WakeForm {
                project_id: project_id.clone(),
                ..Default::default()
            });
        }
        self.wake_form.as_mut().expect("just set")
    }

    /// Connection card → Wake URL → Save: an empty URL clears the URL and the header value
    /// together; an empty header value with a URL keeps the stored value (it is never shown
    /// back, so leaving the field blank means "unchanged").
    pub fn save_wake_url(
        &mut self,
        project_id: Id,
        url: String,
        auth: String,
        cx: &mut Context<Self>,
    ) {
        let url = url.trim().to_owned();
        let auth = auth.trim().to_owned();
        let (url, auth) = if url.is_empty() {
            (Some(String::new()), Some(String::new()))
        } else {
            (Some(url), (!auth.is_empty()).then_some(auth))
        };
        let form = self.wake_form_mut(&project_id);
        form.saving = true;
        form.note = None;
        self.command(ClientCommand::OutsideAgentWakeSet {
            project_id,
            url,
            auth,
        });
        cx.notify();
    }

    /// Connection card → Wake URL → Send test wake.
    pub fn send_test_wake(&mut self, project_id: Id, cx: &mut Context<Self>) {
        let form = self.wake_form_mut(&project_id);
        if form.sending {
            return;
        }
        form.sending = true;
        form.note = None;
        self.command(ClientCommand::OutsideAgentWakeTest { project_id });
        cx.notify();
    }

    /// Grok / iPhone "Fork": a new agent with the same look and workspace
    /// context (files + memory). Own agents only; chat history stays empty.
    pub fn duplicate_project(&mut self, project_id: Id, cx: &mut Context<Self>) {
        let Some(src) = self
            .project(&project_id)
            .filter(|p| !p.shared && !p.is_people())
        else {
            return;
        };
        let name = Self::unique_copy_name(&src.name, &self.projects);
        let description = src.description.clone();
        let color = Some(src.color.clone());
        // Keep the source's tree colour even when it was the automatic
        // (id-derived) one; the fork's own id would pick a different swatch.
        let avatar_color_index = Some(crate::ui::avatars::index_for(
            &src.id,
            src.avatar_color_override.or(src.avatar_color_index),
        ));
        // D92: the fork keeps the source's leaf and colour too — or its neutral disc.
        let leaf = Some(if crate::ui::avatars::leaf::is_neutral(src.avatar_leaf) {
            crate::ui::avatars::neutral_pick()
        } else {
            crate::ui::avatars::project_leaf(src)
        });
        self.awaiting_new_project = true;
        self.command(ClientCommand::CreateProject {
            name,
            avatar_color_index,
            leaf,
            picture: None,
            source_project_id: Some(project_id),
            description,
            color,
        });
        self.push_notice("Forking…");
        cx.notify();
    }

    /// Session fork (#1257): a new chat of the **same** agent carrying this
    /// chat's transcript — never a new project (that is `duplicate_project`,
    /// reached only from the sidebar's agent row). The copy shows at once
    /// under a local id and opens; `ChatCreated` swaps the relay's id in and
    /// the box fills the room with the real history and the model's context.
    pub fn fork_chat(&mut self, chat_id: Id, cx: &mut Context<Self>) {
        if !self.multi_chat {
            return;
        }
        let Some(project) = self.project_of_chat(&chat_id).cloned() else {
            return;
        };
        let Some(source) = self
            .tabs(&project.id)
            .iter()
            .find(|c| c.id == chat_id)
            .cloned()
        else {
            return;
        };
        let Some(chat) = forked_chat(&source, &project, new_id()) else {
            return;
        };
        let local_id = chat.id.clone();
        let title = chat.title.clone();
        let messages = self.messages_of(&chat_id).to_vec();
        self.chats.entry(project.id.clone()).or_default().push(chat);
        self.messages.insert(local_id.clone(), messages);
        self.chat_ui.insert(local_id.clone(), ChatUi::default());
        self.active_tab.insert(project.id.clone(), local_id.clone());
        self.last_chats.insert(project.id.clone(), local_id.clone());
        self.command(ClientCommand::ForkChat {
            project_id: project.id,
            source_chat_id: chat_id,
            chat_id: local_id.clone(),
            title,
        });
        self.command(ClientCommand::OpenChat { chat_id: local_id });
        cx.notify();
    }

    /// Soft-delete an owned agent from every list (`DELETE /projects/{id}`).
    pub fn delete_project(&mut self, project_id: Id, cx: &mut Context<Self>) {
        let Some(p) = self.project(&project_id) else {
            return;
        };
        if p.shared || p.is_people() {
            return;
        }
        if self.selected_project.as_deref() == Some(project_id.as_str()) {
            self.selected_project = None;
        }
        if let Some(tabs) = self.chats.remove(&project_id) {
            for c in tabs {
                self.messages.remove(&c.id);
                self.search_index.remove_chat(&c.id);
                self.chat_ui.remove(&c.id);
                self.archived_chats.remove(&c.id);
            }
        }
        self.active_tab.remove(&project_id);
        self.last_chats.remove(&project_id);
        self.unpin_if_pinned(&project_id);
        self.projects.retain(|p| p.id != project_id);
        self.muted.remove(&project_id);
        self.command(ClientCommand::DeleteProject { project_id });
        cx.notify();
    }

    /// Delete a people chat I own (`DELETE /chats/{id}`).
    pub fn delete_people_chat(&mut self, chat_id: Id, cx: &mut Context<Self>) {
        if !self.project(&chat_id).is_some_and(|p| p.is_people()) {
            return;
        }
        if self.selected_project.as_deref() == Some(chat_id.as_str()) {
            self.selected_project = None;
        }
        self.messages.remove(&chat_id);
        self.search_index.remove_chat(&chat_id);
        self.chat_ui.remove(&chat_id);
        self.chats.remove(&chat_id);
        self.archived_chats.remove(&chat_id);
        self.unpin_if_pinned(&chat_id);
        self.projects.retain(|p| p.id != chat_id);
        self.muted.remove(&chat_id);
        self.command(ClientCommand::DeletePeopleChat { chat_id });
        self.select_next_if_none(cx);
        cx.notify();
    }

    /// Permanently close a non-pinned chat tab (drops it from Archived too).
    pub fn delete_chat_tab(&mut self, chat_id: Id, cx: &mut Context<Self>) {
        if !self.multi_chat {
            return;
        }
        let Some(project_id) = self.project_of_chat(&chat_id).map(|p| p.id.clone()) else {
            return;
        };
        if self
            .tabs(&project_id)
            .iter()
            .any(|c| c.id == chat_id && c.is_pinned)
        {
            return;
        }
        self.archived_chats.remove(&chat_id);
        if let Some(tabs) = self.chats.get_mut(&project_id) {
            tabs.retain(|c| c.id != chat_id);
        }
        self.messages.remove(&chat_id);
        self.search_index.remove_chat(&chat_id);
        self.chat_ui.remove(&chat_id);
        if self.active_tab.get(&project_id) == Some(&chat_id) {
            let pick = self
                .visible_tabs(&project_id)
                .into_iter()
                .next()
                .map(|c| c.id.clone());
            if let Some(pick) = pick {
                self.active_tab.insert(project_id.clone(), pick.clone());
                self.last_chats.insert(project_id.clone(), pick.clone());
                self.command(ClientCommand::OpenChat { chat_id: pick });
            } else {
                self.active_tab.remove(&project_id);
            }
        }
        self.command(ClientCommand::CloseChat {
            project_id,
            chat_id,
        });
        cx.notify();
    }

    /// "`Name` copy", then "`Name` copy 2"… so the relay's unique-name check passes.
    fn unique_copy_name(name: &str, projects: &[Project]) -> String {
        let owned: HashSet<&str> = projects
            .iter()
            .filter(|p| !p.shared && !p.is_people())
            .map(|p| p.name.as_str())
            .collect();
        let base = format!("{name} copy");
        if !owned.contains(base.as_str()) {
            return base;
        }
        for i in 2..=99 {
            let candidate = format!("{name} copy {i}");
            if !owned.contains(candidate.as_str()) {
                return candidate;
            }
        }
        format!("{name} copy {}", &new_id()[..4])
    }

    pub fn fetch_blob(&mut self, key: String, save: bool, cx: &mut Context<Self>) {
        let Some(project_id) = self.selected_project.clone() else {
            return;
        };
        self.command(ClientCommand::FetchBlob {
            project_id,
            key,
            save,
        });
        cx.notify();
    }

    /// Copy of a whole message — the bubble menu, control-click, ⌘C on a whole
    /// bubble ([feedback copy-image-messages-mac-2026-10-27]): its body
    /// (`Message::body`, the accessor Forward reads too) as [`crate::pasteboard`] lays
    /// it out. Words alone land at once; a message with pictures or files first fetches
    /// their original bytes from the relay (never the thumbnail) and writes the
    /// pasteboard when they are in. Says what happened when nothing could be copied.
    pub fn copy_message(&mut self, chat_id: &Id, message_id: &Id, cx: &mut Context<Self>) {
        let Some(message) = self
            .messages
            .get(chat_id)
            .and_then(|ms| ms.iter().find(|m| m.id == *message_id))
        else {
            return;
        };
        let Some(body) = message.body() else {
            self.push_notice(if message.has_uploading_attachment() {
                "Still uploading — try again in a moment"
            } else {
                "Nothing to copy"
            });
            cx.notify();
            return;
        };
        let slot = self.space_of_chat(chat_id);
        self.copy_body(slot, body, cx);
    }

    /// Copy of one attachment on its own — the media viewer's Copy, the Library's
    /// row menu: the picture (or file) without the message's words.
    pub fn copy_attachment(&mut self, slot: Id, attachment: &Attachment, cx: &mut Context<Self>) {
        if !attachment.is_on_relay() {
            self.push_notice(if attachment.is_uploading() {
                "Still uploading — try again in a moment"
            } else {
                "Nothing to copy"
            });
            cx.notify();
            return;
        }
        let body = MessageBody {
            text: String::new(),
            attachments: vec![attachment.clone()],
            charts: Vec::new(),
        };
        self.copy_body(slot, body, cx);
    }

    fn copy_body(&mut self, slot: Id, body: MessageBody, cx: &mut Context<Self>) {
        if body.attachments.is_empty() {
            let items =
                crate::pasteboard::plan(&body.text, &[], &crate::pasteboard::copied_files_dir());
            crate::pasteboard::write(&items, cx);
            self.copy_request = None;
            return;
        }
        self.copy_request_seq += 1;
        let request = self.copy_request_seq;
        let keys: Vec<String> = body
            .attachments
            .iter()
            .filter_map(|a| a.blob_ref.clone())
            .collect();
        self.copy_request = Some(CopyRequest { id: request, body });
        self.command(ClientCommand::FetchCopyBlobs {
            project_id: slot,
            request,
            keys,
        });
        cx.notify();
    }

    /// The blobs of a Copy came back: lay the items out and write the pasteboard.
    fn finish_copy(
        &mut self,
        request: u64,
        blobs: Vec<Option<crate::sync::CopyBlob>>,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self.copy_request.take_if(|p| p.id == request) else {
            return;
        };
        let fetched: Vec<crate::pasteboard::Fetched> = pending
            .body
            .attachments
            .iter()
            .zip(blobs)
            .filter_map(|(attachment, blob)| {
                let blob = blob?;
                // The relay's content-type unless it is the generic one; then the row's.
                let generic =
                    blob.mime.is_empty() || blob.mime.starts_with("application/octet-stream");
                Some(crate::pasteboard::Fetched {
                    name: attachment.name.clone(),
                    mime: if generic {
                        attachment.mime.clone()
                    } else {
                        blob.mime.clone()
                    },
                    bytes: blob.bytes.as_ref().clone(),
                })
            })
            .collect();
        let missing = pending.body.attachments.len() - fetched.len();
        let items = crate::pasteboard::plan(
            &pending.body.text,
            &fetched,
            &crate::pasteboard::copied_files_dir(),
        );
        let written = crate::pasteboard::write(&items, cx);
        if !written {
            self.push_notice("Couldn't copy — the picture didn't download. Try again.");
        } else if missing > 0 {
            self.push_notice(if missing == 1 {
                "Copied — one picture didn't download".to_owned()
            } else {
                format!("Copied — {missing} pictures didn't download")
            });
        } else {
            let pictures = fetched.len();
            self.push_notice(match (pictures, pending.body.text.is_empty()) {
                (1, true) => "Copied the picture".to_owned(),
                (1, false) => "Copied the picture and its text".to_owned(),
                (n, true) => format!("Copied {n} pictures"),
                (n, false) => format!("Copied {n} pictures and the text"),
            });
        }
        cx.notify();
    }

    /// Ask the source for the site icon of every named link in `messages` that has not been
    /// asked for yet ([feedback link-icons-mac-2026-10-25]): the hosts of `[Name](https://…)`
    /// in prose, lists and table cells of agent and people rows alike.
    fn request_link_icons_for(&mut self, messages: &[Message]) {
        for message in messages {
            for part in &message.parts {
                let Part::Text { text, .. } = part else {
                    continue;
                };
                for host in crate::ui::markdown::link_hosts(text) {
                    self.want_link_icon(host);
                }
            }
        }
    }

    /// Ask the source for one host's site icon, once per session (the bubbles' named links,
    /// the link tiles of the Links tab, the Library and search).
    pub(crate) fn want_link_icon(&mut self, host: String) {
        if host.is_empty() {
            return;
        }
        if self.link_icons_requested.insert(host.clone()) {
            self.command(ClientCommand::FetchLinkIcon { host });
        }
    }

    /// The fetched icon for a host and its tone, once the relay answered with a picture.
    pub fn link_icon(&self, host: &str) -> Option<(&Arc<RenderImage>, crate::ui::link_tile::Tone)> {
        let image = self.link_icons.get(host)?.as_ref()?;
        let tone = self.link_icon_tones.get(host).copied().unwrap_or_default();
        Some((image, tone))
    }

    /// Ask the source for the bytes of every image attachment in a chat that has no thumbnail yet.
    fn request_thumbnails(&mut self, chat_id: &Id) {
        let messages = self.messages.get(chat_id).cloned().unwrap_or_default();
        self.request_thumbnails_for(chat_id, &messages);
    }

    /// Reuse the optimistic `local:…` decode when a server frame replaces the
    /// echo: copy it onto the real blob key, or keep the local key when the
    /// upload left `blob_ref` empty so the card still shows a picture.
    fn carry_local_previews(&mut self, old: &Message, new: &mut Message) {
        let old_keys: Vec<Option<String>> = old
            .parts
            .iter()
            .filter_map(|p| match p {
                Part::Attachment { attachment, .. } => Some(attachment.blob_ref.clone()),
                _ => None,
            })
            .collect();
        let mut i = 0usize;
        for part in &mut new.parts {
            let Part::Attachment { attachment, .. } = part else {
                continue;
            };
            let old_key = old_keys.get(i).cloned().flatten();
            i += 1;
            let Some(old_key) = old_key else {
                continue;
            };
            if !old_key.starts_with("local:") {
                continue;
            }
            let Some(image) = self.thumbnails.get(&old_key).cloned() else {
                continue;
            };
            match attachment.blob_ref.clone() {
                Some(new_key) if new_key != old_key => {
                    self.thumbnails.entry(new_key.clone()).or_insert(image);
                    if let Some(t) = self.thumbnails_trimmed.get(&old_key).cloned() {
                        self.thumbnails_trimmed.entry(new_key.clone()).or_insert(t);
                    }
                    self.thumbnails_requested.insert(new_key.clone());
                    self.thumbnails_failed.remove(&new_key);
                }
                None => {
                    attachment.blob_ref = Some(old_key);
                }
                Some(_) => {}
            }
        }
    }

    fn request_thumbnails_for(&mut self, chat_id: &Id, messages: &[Message]) {
        // Only the chat's own space: guessing `selected_project` for a background
        // upsert (or a chat not yet in the map) hits the wrong blob slot → HTTP 404
        // and a weekly `mac:thumbnail:* http 404` diagnostic. Snapshot / open retries
        // once the chat is listed.
        let Some(project_id) = self
            .chats
            .iter()
            .find(|(_, tabs)| tabs.iter().any(|c| &c.id == chat_id))
            .map(|(pid, _)| pid.clone())
        else {
            return;
        };
        for message in messages {
            for part in &message.parts {
                // A link preview's picture lives in the chat's slot like an attachment's; its
                // site icon too, and a host without one is asked of the relay (the shared tile).
                let key = match part {
                    Part::LinkPreview { preview, .. } => {
                        if let Some(icon) = preview
                            .site_icon
                            .as_ref()
                            .filter(|i| !i.blob_ref.is_empty())
                        {
                            if self.thumbnails_requested.insert(icon.blob_ref.clone()) {
                                self.command(ClientCommand::FetchThumbnail {
                                    project_id: project_id.clone(),
                                    key: icon.blob_ref.clone(),
                                });
                            }
                        } else if let Some(host) =
                            preview.site_host.clone().filter(|h| !h.is_empty())
                        {
                            if crate::ui::link_tile::Brand::for_host(&host).is_none() {
                                self.want_link_icon(host);
                            }
                        }
                        match preview.image.as_ref().map(|i| i.blob_ref.clone()) {
                            Some(k) if !k.is_empty() => Some(k),
                            _ => None,
                        }
                    }
                    Part::Attachment { attachment, .. }
                        if matches!(
                            attachment.kind(),
                            AttachmentKind::Image | AttachmentKind::Sticker
                        ) =>
                    {
                        attachment.preview_key()
                    }
                    _ => None,
                };
                let Some(key) = key else {
                    continue;
                };
                // Optimistic send stores a decoded preview under `local:…`.
                if key.starts_with("local:") {
                    continue;
                }
                if !self.thumbnails_requested.insert(key.clone()) {
                    continue;
                }
                self.command(ClientCommand::FetchThumbnail {
                    project_id: project_id.clone(),
                    key,
                });
            }
        }
    }

    /// Download an attachment to `~/Downloads/<name>`; the source answers with `AttachmentSaved`
    /// (or a dismissible failure notice). No progress bubble — those sat in the chat like messages.
    pub fn download_attachment(&mut self, key: String, name: String, cx: &mut Context<Self>) {
        let Some(project_id) = self.selected_project.clone() else {
            return;
        };
        self.command(ClientCommand::DownloadAttachment {
            project_id,
            key,
            name,
        });
        cx.notify();
    }

    // ----- connectors (F1) -------------------------------------------------

    pub fn refresh_connectors(&mut self, cx: &mut Context<Self>) {
        self.command(ClientCommand::RefreshConnectors);
        cx.notify();
    }

    pub fn connect_connector(
        &mut self,
        provider: String,
        token: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.push_notice(if token.is_some() {
            format!("Saving the {provider} token on the relay…")
        } else {
            format!("Starting {provider} sign-in…")
        });
        self.command(ClientCommand::ConnectorConnect { provider, token });
        cx.notify();
    }

    pub fn disconnect_connector(&mut self, id: Id, cx: &mut Context<Self>) {
        self.connectors.items.retain(|c| c.id != id);
        self.command(ClientCommand::ConnectorDisconnect { id });
        cx.notify();
    }

    // ----- this mac (F9) ---------------------------------------------------

    pub fn local_refresh(&mut self, cx: &mut Context<Self>) {
        self.command(ClientCommand::LocalRefresh);
        cx.notify();
    }

    pub fn local_set_allow_reads(&mut self, device_id: Id, allow: bool, cx: &mut Context<Self>) {
        if let Some(d) = self
            .local
            .devices
            .iter_mut()
            .find(|d| d.device_id == device_id)
        {
            d.allow_reads = allow;
        }
        self.command(ClientCommand::LocalSetAllowReads { device_id, allow });
        cx.notify();
    }

    pub fn local_forget(&mut self, device_id: Id, cx: &mut Context<Self>) {
        self.local.devices.retain(|d| d.device_id != device_id);
        self.command(ClientCommand::LocalForget { device_id });
        cx.notify();
    }

    // ----- secrets (D83) ---------------------------------------------------

    pub fn secrets_refresh(&mut self, cx: &mut Context<Self>) {
        self.command(ClientCommand::SecretsRefresh);
        cx.notify();
    }

    // ----- devices (Settings → Devices, Oct 23) ---------------------------

    /// Settings → Devices opened: ask the relay for the list (the page shows
    /// "Loading…" until the answer, the last list meanwhile).
    pub fn devices_refresh(&mut self, cx: &mut Context<Self>) {
        self.devices.error = None;
        self.command(ClientCommand::DevicesRefresh);
        cx.notify();
    }

    /// Sign another device out. The current device never goes through here
    /// (its row offers Sign out instead), so the user is never stranded.
    pub fn revoke_device(&mut self, id: Id, cx: &mut Context<Self>) {
        if !crate::ui::devices::remove_other(&mut self.devices.items, &id) {
            return;
        }
        self.command(ClientCommand::DeviceRevoke { id });
        cx.notify();
    }

    pub fn project_secrets_refresh(&mut self, project_id: Id, cx: &mut Context<Self>) {
        self.command(ClientCommand::ProjectSecretsRefresh { project_id });
        cx.notify();
    }

    /// Switch a standing shell grant off (`DELETE /projects/{id}/shell-grants/{grant}`). The
    /// row goes at once; the relay's list confirms it. The Permissions card that showed the
    /// switches left the agent pane on 2026-10-25 ([feedback
    /// agent-details-remove-sections-mac-2026-10-25]); the call stays for the next way in.
    #[allow(dead_code)]
    pub fn shell_grant_revoke(&mut self, project_id: Id, grant_id: String, cx: &mut Context<Self>) {
        if let Some(list) = self.shell_grants.get_mut(&project_id) {
            list.retain(|g| g.id != grant_id);
        }
        self.command(ClientCommand::ShellGrantRevoke {
            project_id,
            grant_id,
        });
        cx.notify();
    }

    pub fn secret_add(&mut self, item: NewSecret, cx: &mut Context<Self>) {
        self.push_notice(format!("Saving {} on the relay…", item.name));
        self.command(ClientCommand::SecretAdd { item });
        cx.notify();
    }

    pub fn secret_patch(&mut self, id: Id, body: serde_json::Value, cx: &mut Context<Self>) {
        if let Some(s) = self.secrets.secrets.iter_mut().find(|s| s.id == id) {
            if let Some(a) = body.get("ask_every_time").and_then(|v| v.as_bool()) {
                s.ask_every_time = a;
                if a {
                    s.inject_env = false;
                }
            }
            if let Some(i) = body.get("inject_env").and_then(|v| v.as_bool()) {
                s.inject_env = i;
                if i {
                    s.ask_every_time = false;
                }
            }
            if let Some(n) = body.get("note").and_then(|v| v.as_str()) {
                s.note = n.to_string();
            }
            if let Some(o) = body.get("others_ask").and_then(|v| v.as_str()) {
                s.others_ask = o.to_string();
            }
        }
        self.command(ClientCommand::SecretPatch { id, body });
        cx.notify();
    }

    pub fn secret_replace_value(
        &mut self,
        id: Id,
        body: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        self.command(ClientCommand::SecretReplaceValue { id, body });
        cx.notify();
    }

    pub fn secret_delete(&mut self, id: Id, cx: &mut Context<Self>) {
        self.secrets.secrets.retain(|s| s.id != id);
        self.command(ClientCommand::SecretDelete { id });
        cx.notify();
    }

    /// One switch: secret × agent. Optimistic; the relay's view follows.
    pub fn secret_access(
        &mut self,
        id: Id,
        project_id: Id,
        enabled: bool,
        note: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(s) = self.secrets.secrets.iter_mut().find(|s| s.id == id) {
            if let Some(a) = s.agents.iter_mut().find(|a| a.project_id == project_id) {
                a.enabled = enabled;
            }
        }
        if let Some(ps) = self.project_secrets.get_mut(&project_id) {
            if let Some(i) = ps
                .items
                .iter_mut()
                .find(|i| i.secret.as_ref().is_some_and(|s| s.id == id))
            {
                i.enabled = enabled;
            }
        }
        self.command(ClientCommand::SecretAccess {
            id,
            project_id,
            enabled,
            note,
        });
        cx.notify();
    }

    /// One People row's switch or note. Optimistic; the relay's view follows.
    pub fn secret_person(
        &mut self,
        id: Id,
        user_id: Id,
        enabled: bool,
        note: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(s) = self.secrets.secrets.iter_mut().find(|s| s.id == id) {
            if let Some(d) = s.directors.iter_mut().find(|d| d.user_id == user_id) {
                d.enabled = enabled;
                d.disabled_at = (!enabled).then(|| now_ms() as i64);
                if let Some(n) = &note {
                    d.note = n.clone();
                }
            }
        }
        self.command(ClientCommand::SecretPerson {
            id,
            user_id,
            enabled,
            note,
        });
        cx.notify();
    }

    /// People picked in the member picker join the list (rows on, no note yet).
    pub fn secret_people_add(&mut self, id: Id, handles: Vec<String>, cx: &mut Context<Self>) {
        if handles.is_empty() {
            return;
        }
        self.command(ClientCommand::SecretPeopleAdd { id, handles });
        cx.notify();
    }

    pub fn secret_person_remove(&mut self, id: Id, user_id: Id, cx: &mut Context<Self>) {
        if let Some(s) = self.secrets.secrets.iter_mut().find(|s| s.id == id) {
            s.directors.retain(|d| d.user_id != user_id);
        }
        self.command(ClientCommand::SecretPersonRemove { id, user_id });
        cx.notify();
    }

    pub fn secret_revoke_all(&mut self, id: Id, cx: &mut Context<Self>) {
        if let Some(s) = self.secrets.secrets.iter_mut().find(|s| s.id == id) {
            for a in &mut s.agents {
                a.enabled = false;
            }
        }
        self.command(ClientCommand::SecretRevokeAll { id });
        cx.notify();
    }

    /// The kill switch. Optimistic: the switch flips now; the relay confirms.
    pub fn secrets_pause(&mut self, paused: bool, cx: &mut Context<Self>) {
        self.secrets.paused = paused;
        self.command(ClientCommand::SecretsPause { paused });
        cx.notify();
    }

    /// `once | chat | always | deny` on a pending request.
    pub fn secret_decide(&mut self, request_id: Id, decision: String, cx: &mut Context<Self>) {
        if let Some(r) = self
            .secrets
            .requests
            .iter_mut()
            .find(|r| r.id == request_id)
        {
            r.status = match decision.as_str() {
                "deny" => "denied",
                "once" => "allowed_once",
                "chat" => "allowed_chat",
                _ => "allowed_always",
            }
            .into();
            r.decided_at = Some(now_ms() as i64);
        }
        for ps in self.project_secrets.values_mut() {
            ps.requests.retain(|r| r.id != request_id);
        }
        self.command(ClientCommand::SecretDecide {
            request_id,
            decision,
        });
        cx.notify();
    }

    pub fn vault_connect(
        &mut self,
        token: String,
        agents: Vec<Id>,
        note: String,
        cx: &mut Context<Self>,
    ) {
        self.push_notice("Checking the token with 1Password…".to_string());
        self.command(ClientCommand::VaultConnect {
            token,
            agents,
            note,
        });
        cx.notify();
    }

    /// One vault item's real value into (or out of) the environment of every agent with Access.
    pub fn vault_item_env(&mut self, id: Id, reference: String, on: bool, cx: &mut Context<Self>) {
        if let Some(v) = self.secrets.vaults.iter_mut().find(|v| v.id == id) {
            for it in v.items.iter_mut() {
                if it["ref"].as_str() == Some(reference.as_str()) {
                    it["inject_env"] = serde_json::json!(on);
                }
            }
            v.env_items = v
                .items
                .iter()
                .filter(|it| it["inject_env"].as_bool().unwrap_or(false))
                .count() as i64;
        }
        self.command(ClientCommand::VaultItemEnv { id, reference, on });
        cx.notify();
    }

    pub fn vault_access(&mut self, id: Id, project_id: Id, enabled: bool, cx: &mut Context<Self>) {
        if let Some(v) = self.secrets.vaults.iter_mut().find(|v| v.id == id) {
            if let Some(a) = v.agents.iter_mut().find(|a| a.project_id == project_id) {
                a.enabled = enabled;
            }
        }
        if let Some(ps) = self.project_secrets.get_mut(&project_id) {
            if let Some(i) = ps
                .items
                .iter_mut()
                .find(|i| i.vault.as_ref().is_some_and(|v| v.id == id))
            {
                i.enabled = enabled;
            }
        }
        self.command(ClientCommand::VaultAccess {
            id,
            project_id,
            enabled,
        });
        cx.notify();
    }

    pub fn vault_replace_token(&mut self, id: Id, token: String, cx: &mut Context<Self>) {
        self.push_notice("Checking the token with 1Password…".to_string());
        self.command(ClientCommand::VaultReplaceToken { id, token });
        cx.notify();
    }

    pub fn vault_disconnect(&mut self, id: Id, cx: &mut Context<Self>) {
        self.secrets.vaults.retain(|v| v.id != id);
        self.command(ClientCommand::VaultDisconnect { id });
        cx.notify();
    }

    // ----- routines (F3) ---------------------------------------------------

    pub fn routines_of(&self, project_id: &str) -> &[Routine] {
        self.routines
            .get(project_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn routine_runs_of(&self, routine_id: &str) -> &[RoutineRun] {
        self.routine_runs
            .get(routine_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn routines_refresh(&mut self, cx: &mut Context<Self>) {
        // Routines are per agent project. A people space or a selection the
        // registry no longer has would 404 ("no such project") — skip quietly.
        if let Some(project_id) = self.selected_project.clone() {
            if self.project(&project_id).is_some_and(|p| !p.is_people()) {
                self.command(ClientCommand::RoutinesRefresh { project_id });
            }
        }
        cx.notify();
    }

    pub fn routine_create(
        &mut self,
        name: String,
        prompt: String,
        chat_id: Id,
        trigger: RoutineTrigger,
        cx: &mut Context<Self>,
    ) {
        let Some(project_id) = self.selected_project.clone() else {
            return;
        };
        self.command(ClientCommand::RoutineCreate {
            project_id,
            name,
            prompt,
            chat_id,
            trigger,
        });
        cx.notify();
    }

    pub fn routine_set_enabled(&mut self, routine_id: Id, enabled: bool, cx: &mut Context<Self>) {
        let Some(project_id) = self.selected_project.clone() else {
            return;
        };
        if let Some(r) = self
            .routines
            .get_mut(&project_id)
            .and_then(|v| v.iter_mut().find(|r| r.id == routine_id))
        {
            r.enabled = enabled;
        }
        self.command(ClientCommand::RoutineSetEnabled {
            project_id,
            routine_id,
            enabled,
        });
        cx.notify();
    }

    pub fn routine_run_now(&mut self, routine_id: Id, cx: &mut Context<Self>) {
        let Some(project_id) = self.selected_project.clone() else {
            return;
        };
        self.command(ClientCommand::RoutineRunNow {
            project_id,
            routine_id,
        });
        self.push_notice("Routine queued");
        cx.notify();
    }

    pub fn routine_load_runs(&mut self, routine_id: Id, cx: &mut Context<Self>) {
        let Some(project_id) = self.selected_project.clone() else {
            return;
        };
        self.command(ClientCommand::RoutineRuns {
            project_id,
            routine_id,
        });
        cx.notify();
    }

    pub fn routine_delete(&mut self, routine_id: Id, cx: &mut Context<Self>) {
        let Some(project_id) = self.selected_project.clone() else {
            return;
        };
        if let Some(v) = self.routines.get_mut(&project_id) {
            v.retain(|r| r.id != routine_id);
        }
        self.command(ClientCommand::RoutineDelete {
            project_id,
            routine_id,
        });
        cx.notify();
    }

    /// History "open": switch to the chat that holds the reply.
    pub fn open_chat_of_routine(&mut self, routine_id: &str, cx: &mut Context<Self>) {
        let chat_id = self
            .selected_project
            .as_deref()
            .and_then(|p| self.routines_of(p).iter().find(|r| r.id == routine_id))
            .map(|r| r.chat_id.clone());
        if let Some(chat_id) = chat_id {
            self.select_tab(chat_id, cx);
        }
    }

    // ----- skills (F5) -------------------------------------------------------

    pub fn refresh_skills(&mut self, cx: &mut Context<Self>) {
        self.command(ClientCommand::SkillsRefresh);
        cx.notify();
    }

    pub fn save_skill(&mut self, skill: Skill, cx: &mut Context<Self>) {
        // Optimistic: the list shows the edit at once; the source's `Skills`
        // event replaces it with the stored version.
        match self
            .skills
            .iter_mut()
            .find(|s| s.id == skill.id && !skill.id.is_empty())
        {
            Some(existing) => *existing = skill.clone(),
            None => self.skills.push(skill.clone()),
        }
        self.command(ClientCommand::SkillSave { skill });
        cx.notify();
    }

    pub fn delete_skill(&mut self, id: Id, cx: &mut Context<Self>) {
        self.skills.retain(|s| s.id != id);
        self.command(ClientCommand::SkillDelete { id });
        cx.notify();
    }

    // ----- channels (F7) ---------------------------------------------------

    pub fn refresh_channels(&mut self, cx: &mut Context<Self>) {
        self.command(ClientCommand::RefreshChannels);
        cx.notify();
    }

    /// Mint a pairing code for the selected project (its pinned chat).
    pub fn new_channel_pairing(&mut self, cx: &mut Context<Self>) {
        let Some(project_id) = self.selected_project.clone() else {
            self.push_notice("Select a project first.");
            cx.notify();
            return;
        };
        self.command(ClientCommand::ChannelPairNew {
            project_id,
            chat_id: None,
        });
        cx.notify();
    }

    pub fn unbind_channel(&mut self, id: Id, cx: &mut Context<Self>) {
        self.channels.bindings.retain(|b| b.id != id);
        self.command(ClientCommand::ChannelUnbind { id });
        cx.notify();
    }

    pub fn selected_doc(&self) -> Option<&ProjectDocView> {
        self.selected_project
            .as_deref()
            .and_then(|id| self.project_docs.get(id))
    }

    pub fn box_state(&self) -> Option<BoxState> {
        self.selected().map(|p| p.state)
    }
}

impl AppState {
    // ----- calls v1 ---------------------------------------------------------

    /// Start a call in the selected chat (or join the live one).
    pub fn start_call_in_chat(&mut self, kind: CallKind, cx: &mut Context<Self>) {
        let Some(space_id) = self.selected_project.clone() else {
            return;
        };
        if self.my_call.is_some() {
            return;
        }
        if crate::draft_dm::is_draft(&space_id) {
            self.push_notice("Say hi first, then call.".to_string());
            cx.notify();
            return;
        }
        // Already a live call here: join it instead of starting another.
        if let Some(call) = self.calls_in_chat.get(&space_id).cloned() {
            self.join_call(call.call_id.clone(), cx);
            return;
        }
        // People chats are their own room; a project space calls in its open (or primary) chat,
        // where the project's agent joins through the voice worker (Calls Phase 2).
        let is_people = self.project(&space_id).is_some_and(|p| p.is_people());
        let chat_id = if is_people {
            space_id
        } else {
            match self
                .active_chat_id()
                .filter(|c| self.tabs(&space_id).iter().any(|ch| &ch.id == c))
                .or_else(|| self.primary_chat(&space_id).map(|c| c.id.clone()))
            {
                Some(c) => c,
                None => return,
            }
        };
        self.my_call = Some(ActiveCall {
            call: Call {
                chat_id: chat_id.clone(),
                kind,
                ..Default::default()
            },
            status: CallStatus::Connecting,
            muted: false,
            camera: kind == CallKind::Video,
            participants: Vec::new(),
            local_video: None,
            joined_at: now_ms(),
            expanded: true,
            notice: None,
            outgoing: true,
            reachable: None,
            agent_seeing: false,
        });
        self.command(ClientCommand::StartCall { chat_id, kind });
        self.sync_tones();
        cx.notify();
    }

    pub fn join_call(&mut self, call_id: String, cx: &mut Context<Self>) {
        if self.my_call.is_some() {
            return;
        }
        self.incoming_since = None;
        let known = self
            .calls_in_chat
            .values()
            .find(|c| c.call_id == call_id)
            .cloned()
            .unwrap_or_default();
        self.my_call = Some(ActiveCall {
            camera: known.kind == CallKind::Video,
            call: known,
            status: CallStatus::Connecting,
            muted: false,
            participants: Vec::new(),
            local_video: None,
            joined_at: now_ms(),
            expanded: true,
            notice: None,
            outgoing: false,
            reachable: None,
            agent_seeing: false,
        });
        self.incoming_call = None;
        self.command(ClientCommand::JoinCall { call_id });
        cx.notify();
    }

    pub fn accept_incoming(&mut self, cx: &mut Context<Self>) {
        let Some((call, _)) = self.incoming_call.take() else {
            return;
        };
        if self.my_call.is_some() {
            self.leave_call(cx);
        }
        self.select_project(call.space_id().to_owned(), cx);
        self.join_call(call.call_id, cx);
    }

    pub fn decline_incoming(&mut self, cx: &mut Context<Self>) {
        if let Some((call, _)) = self.incoming_call.take() {
            notify::withdraw_call(&call.call_id);
        }
        self.incoming_since = None;
        self.sync_tones();
        cx.notify();
    }

    /// Ringback while my outgoing call has nobody else in it; ring while the
    /// ring card is up. Idempotent: called after every call state change.
    fn sync_tones(&mut self) {
        let ringback = self
            .my_call
            .as_ref()
            .map(|c| {
                c.outgoing
                    && c.participants.is_empty()
                    && !matches!(c.status, CallStatus::Failed(_))
            })
            .unwrap_or(false);
        match (ringback, self.ringback_tone.is_some()) {
            (true, false) => self.ringback_tone = Some(TonePlayer::start(Tone::Ringback)),
            (false, true) => self.ringback_tone = None,
            _ => {}
        }
        let ring = self.incoming_call.is_some() && self.my_call.is_none();
        match (ring, self.ring_tone.is_some()) {
            (true, false) => self.ring_tone = Some(TonePlayer::start(Tone::Ring)),
            (false, true) => self.ring_tone = None,
            _ => {}
        }
    }

    /// Every notify tick: stop ringing after `RING_TIMEOUT_MS`, give up on an
    /// unanswered outgoing call after `RINGBACK_TIMEOUT_MS`.
    fn tick_calls(&mut self, cx: &mut Context<Self>) {
        if !self.missed_pending.is_empty() {
            let now = now_ms();
            let pending = std::mem::take(&mut self.missed_pending);
            for (missed, since) in pending {
                if now.saturating_sub(since) <= MISSED_PENDING_MS {
                    self.on_call_missed(missed, cx);
                }
            }
        }
        if let Some(since) = self.incoming_since {
            if now_ms().saturating_sub(since) > RING_TIMEOUT_MS {
                self.decline_incoming(cx);
            }
        }
        let give_up = self
            .my_call
            .as_ref()
            .map(|c| {
                c.outgoing
                    && c.participants.is_empty()
                    && now_ms().saturating_sub(c.joined_at) > RINGBACK_TIMEOUT_MS
            })
            .unwrap_or(false);
        if give_up {
            self.leave_call(cx);
            self.push_notice("No answer.");
            cx.notify();
        }
        self.sync_tones();
    }

    pub fn leave_call(&mut self, cx: &mut Context<Self>) {
        let Some(active) = self.my_call.take() else {
            return;
        };
        let _ = self.call_commands.send(CallCmd::Leave);
        if !active.call.call_id.is_empty() {
            self.command(ClientCommand::LeaveCall {
                call_id: active.call.call_id.clone(),
            });
        }
        drop_active_call_images(active, cx);
        self.sync_tones();
        cx.notify();
    }

    pub fn toggle_call_mute(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = self.my_call.as_mut() {
            // Hosted group calls: listen-only until the host invites you to speak.
            if c.call.can_speak == Some(false) {
                return;
            }
            c.muted = !c.muted;
            let _ = self.call_commands.send(CallCmd::SetMuted(c.muted));
            cx.notify();
        }
    }

    pub fn toggle_camera(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = self.my_call.as_mut() {
            c.camera = !c.camera;
            if !c.camera {
                drop_video_frame(cx, swap_frame(&mut c.local_video, None));
                c.agent_seeing = false;
            }
            c.notice = None;
            let _ = self.call_commands.send(CallCmd::SetCamera(c.camera));
            cx.notify();
        }
    }

    pub fn toggle_call_expanded(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = self.my_call.as_mut() {
            c.expanded = !c.expanded;
            cx.notify();
        }
    }

    /// Who I am, as the relay would describe me in a call.
    fn me_sender(&self) -> Sender {
        let account = self.account.clone();
        Sender {
            kind: "user".into(),
            id: self.me_id(),
            name: account.as_ref().map(|a| a.name.clone()).unwrap_or_default(),
            handle: self.me_handle(),
            avatar: None,
            avatar_color: None,
            avatar_color_index: None,
            avatar_leaf: None,
            avatar_leaf_color: None,
            avatar_url: account.map(|a| a.avatar_url).unwrap_or_default(),
        }
    }

    fn on_call_ready(
        &mut self,
        call: Call,
        created: bool,
        reachable: Vec<Reachable>,
        cx: &mut Context<Self>,
    ) {
        self.calls_in_chat
            .insert(call.space_id().to_owned(), call.clone());
        if created {
            let line = format!(
                "{} started a {}",
                self.me_sender().display_name(),
                call.kind.label()
            );
            self.command(ClientCommand::SystemLine {
                chat_id: call.chat_id.clone(),
                text: line,
                call_id: Some(call.call_id.clone()),
            });
        }
        let camera = self
            .my_call
            .as_ref()
            .map(|c| c.camera)
            .unwrap_or(call.kind == CallKind::Video);
        let Some(token) = call.token.clone() else {
            self.push_notice("The relay sent no join token.");
            if let Some(active) = self.my_call.take() {
                drop_active_call_images(active, cx);
            }
            cx.notify();
            return;
        };
        let listen_only = call.can_speak == Some(false);
        let active = self.my_call.get_or_insert_with(|| ActiveCall {
            call: call.clone(),
            status: CallStatus::Connecting,
            muted: listen_only,
            camera,
            participants: Vec::new(),
            local_video: None,
            joined_at: now_ms(),
            expanded: true,
            notice: None,
            outgoing: false,
            reachable: None,
            agent_seeing: false,
        });
        active.call = call.clone();
        active.status = CallStatus::Connecting;
        if listen_only {
            active.muted = true;
        }
        if created {
            active.outgoing = true;
            active.reachable = reachable_line(&reachable);
        }
        let _ = self.call_commands.send(CallCmd::Join {
            url: call.livekit_url.clone(),
            token,
            camera,
        });
        if listen_only {
            let _ = self.call_commands.send(CallCmd::SetMuted(true));
        }
        self.incoming_call = None;
        self.sync_tones();
        cx.notify();
    }

    fn on_call_started(&mut self, call: Call, sender: Sender, cx: &mut Context<Self>) {
        self.calls_in_chat
            .insert(call.space_id().to_owned(), call.clone());
        let in_it = self
            .my_call
            .as_ref()
            .map(|c| c.call.call_id == call.call_id)
            .unwrap_or(false);
        if !in_it && call.is_live() {
            // Ring: the card in the window plus the native banner with Accept / Decline.
            let title = sender.display_name();
            let body = format!("Incoming {}", call.kind.label());
            let image = self
                .avatar_file_for(&sender.avatar_url)
                .or_else(|| self.sender_png_for(&sender));
            notify::deliver_call(&call.call_id, &call.chat_id, &title, &body, image);
            self.incoming_call = Some((call, sender));
            self.incoming_since = Some(now_ms());
        }
        self.sync_tones();
        cx.notify();
    }

    fn on_call_ended(&mut self, ended: CallEnded, cx: &mut Context<Self>) {
        let chat_id = if !ended.chat_id.is_empty() {
            Some(crate::model::space_of(&ended.project_id, &ended.chat_id).to_owned())
        } else {
            self.calls_in_chat
                .iter()
                .find(|(_, c)| c.call_id == ended.call_id)
                .map(|(id, _)| id.clone())
                .or_else(|| {
                    self.my_call
                        .as_ref()
                        .filter(|c| c.call.call_id == ended.call_id)
                        .map(|c| c.call.space_id().to_owned())
                })
        };
        if let Some(chat) = &chat_id {
            if self
                .calls_in_chat
                .get(chat)
                .map(|c| c.call_id == ended.call_id)
                .unwrap_or(false)
            {
                self.calls_in_chat.remove(chat);
            }
        }
        if self
            .incoming_call
            .as_ref()
            .map(|(c, _)| c.call_id == ended.call_id)
            .unwrap_or(false)
        {
            self.incoming_call = None;
            notify::withdraw_call(&ended.call_id);
        }
        if self
            .my_call
            .as_ref()
            .map(|c| c.call.call_id == ended.call_id)
            .unwrap_or(false)
        {
            if let Some(active) = self.my_call.take() {
                drop_active_call_images(active, cx);
            }
            let _ = self.call_commands.send(CallCmd::Leave);
        }
        // Exactly one device writes the end line (contract §1) — and once: the
        // leave answer and the `call_ended` push may both name this device.
        if ended.writer_is_me {
            if let Some(chat) = chat_id {
                let already = self.chat_has_call_line(&chat, &ended.call_id, "Call ended");
                if !already && self.ended_written.insert(ended.call_id.clone()) {
                    self.command(ClientCommand::SystemLine {
                        chat_id: chat,
                        text: call_ended_line(ended.duration_s),
                        call_id: Some(ended.call_id.clone()),
                    });
                }
            }
        }
        if self.incoming_call.is_none() {
            self.incoming_since = None;
        }
        self.sync_tones();
        cx.notify();
    }

    /// A system line for `call_id` starting with `prefix` is already in the chat.
    fn chat_has_call_line(&self, chat_id: &str, call_id: &str, prefix: &str) -> bool {
        self.messages
            .get(chat_id)
            .map(|list| {
                list.iter().any(|m| {
                    m.role == Role::System
                        && m.call_id.as_deref() == Some(call_id)
                        && m.plain_text().starts_with(prefix)
                })
            })
            .unwrap_or(false)
    }

    /// `call_missed` (contract §8): the system line once per call, and a
    /// native "Missed voice call" banner from the caller.
    fn on_call_missed(&mut self, missed: CallMissed, cx: &mut Context<Self>) {
        if missed.call_id.is_empty() {
            return;
        }
        self.calls_in_chat
            .retain(|_, c| c.call_id != missed.call_id);
        if self
            .incoming_call
            .as_ref()
            .map(|(c, _)| c.call_id == missed.call_id)
            .unwrap_or(false)
        {
            self.incoming_call = None;
            self.incoming_since = None;
            notify::withdraw_call(&missed.call_id);
        }
        if !self.messages.contains_key(&missed.chat_id) {
            // The push beat the chat's doc (app just opened): write once it is here.
            if !self
                .missed_pending
                .iter()
                .any(|(m, _)| m.call_id == missed.call_id)
            {
                self.missed_pending.push((missed, now_ms()));
            }
            return;
        }
        let already = self.chat_has_call_line(&missed.chat_id, &missed.call_id, "Missed call");
        if !already && self.missed_written.insert(missed.call_id.clone()) {
            self.command(ClientCommand::SystemLine {
                chat_id: missed.chat_id.clone(),
                text: missed.line(),
                call_id: Some(missed.call_id.clone()),
            });
            let image = self
                .avatar_file_for(&missed.from.avatar_url)
                .or_else(|| self.sender_png_for(&missed.from));
            notify::deliver(&notify::Notification {
                project_id: missed.project_id.clone(),
                title: missed.from.display_name(),
                body: missed.notification_body(),
                count: 1,
                chat_id: Some(missed.chat_id.clone()),
                image,
            });
        }
        self.sync_tones();
        cx.notify();
    }

    fn on_call_info(&mut self, call: Call, cx: &mut Context<Self>) {
        if call.is_live() {
            if let Some(active) = self
                .my_call
                .as_mut()
                .filter(|c| c.call.call_id == call.call_id)
            {
                active.call.participants = call.participants.clone();
                active.call.ended_at = None;
            }
            self.calls_in_chat.insert(call.space_id().to_owned(), call);
        } else {
            self.calls_in_chat.retain(|_, c| c.call_id != call.call_id);
            if self
                .incoming_call
                .as_ref()
                .map(|(c, _)| c.call_id == call.call_id)
                .unwrap_or(false)
            {
                self.incoming_call = None;
                self.incoming_since = None;
                notify::withdraw_call(&call.call_id);
            }
        }
        self.sync_tones();
        cx.notify();
    }

    /// Events from the LiveKit engine.
    fn apply_call_event(&mut self, event: CallEvent, cx: &mut Context<Self>) {
        let my_identity = format!("u_{}", self.me_id());
        let my_handle = self.me_handle();
        let Some(active) = self.my_call.as_mut() else {
            return;
        };
        match event {
            CallEvent::Connecting => {
                if active.status == CallStatus::Connected {
                    active.notice = Some("Reconnecting…".into());
                } else {
                    active.status = CallStatus::Connecting;
                }
            }
            CallEvent::Connected => {
                active.status = CallStatus::Connected;
                if active.notice.as_deref() == Some("Reconnecting…") {
                    active.notice = None;
                }
                // Screenshot rig (the iPhone's ARBOS_FAKE_VISION): pretend the camera is on
                // and the agent reports seeing it, so the §11 badge can be shot without a
                // voice worker. Env-gated only; never set in a normal launch.
                if std::env::var("ARBOS_FAKE_VISION").as_deref() == Ok("1") {
                    active.camera = true;
                    active.agent_seeing = true;
                }
            }
            CallEvent::Participant {
                identity,
                name,
                metadata,
                present,
            } => {
                if present {
                    let mut sender: Sender =
                        serde_json::from_str::<arbos_core::api::SenderDto>(&metadata)
                            .map(|dto| Sender::from(&dto))
                            .unwrap_or_default();
                    if sender.name.is_empty() {
                        sender.name = name;
                    }
                    if sender.id.is_empty() {
                        sender.id = identity
                            .trim_start_matches("u_")
                            .trim_start_matches("a_")
                            .to_string();
                    }
                    // People in the call come from the relay list too: keep the richer one.
                    if sender.avatar_url.is_empty() {
                        if let Some(known) = active
                            .call
                            .participants
                            .iter()
                            .find(|p| p.identity() == identity)
                        {
                            sender = known.clone();
                        }
                    }
                    if !sender.avatar_url.is_empty() {
                        self.want_avatar(sender.avatar_url.clone());
                    }
                    let active = self.my_call.as_mut().unwrap();
                    match active.participant_mut(&identity) {
                        Some(p) => p.sender = sender,
                        None => active.participants.push(CallParticipant {
                            identity,
                            sender,
                            speaking: false,
                            muted: false,
                            video: None,
                        }),
                    }
                } else if let Some(frame) = remove_call_participant(active, &identity) {
                    cx.drop_image(frame, None);
                }
            }
            CallEvent::Video { identity, frame } => {
                let prev = if identity == "local" {
                    swap_frame(&mut active.local_video, Some(frame))
                } else if let Some(p) = active.participant_mut(&identity) {
                    swap_frame(&mut p.video, Some(frame))
                } else {
                    None
                };
                drop_video_frame(cx, prev);
            }
            CallEvent::VideoOff { identity } => {
                let prev = if identity == "local" {
                    swap_frame(&mut active.local_video, None)
                } else if let Some(p) = active.participant_mut(&identity) {
                    swap_frame(&mut p.video, None)
                } else {
                    None
                };
                drop_video_frame(cx, prev);
            }
            CallEvent::Speaking(ids) => {
                for p in active.participants.iter_mut() {
                    p.speaking = ids.contains(&p.identity);
                }
            }
            CallEvent::AgentVision {
                identity,
                seeing,
                seeing_identities,
            } => {
                // §11: about me when it names my identity, or when I am (not) in the
                // full set; only while my camera is on.
                let is_me = |id: &str| id == my_identity || id == "local" || id == my_handle;
                if is_me(&identity) {
                    active.agent_seeing = seeing && active.camera;
                } else if !seeing_identities.is_empty() || identity.is_empty() {
                    let listed = seeing_identities.iter().any(|id| is_me(id));
                    active.agent_seeing = listed && active.camera;
                }
            }
            CallEvent::RemoteMuted { identity, muted } => {
                if let Some(p) = active.participant_mut(&identity) {
                    p.muted = muted;
                }
            }
            CallEvent::CameraUnavailable(why) => {
                active.camera = false;
                drop_video_frame(cx, swap_frame(&mut active.local_video, None));
                active.notice = Some(format!("Camera unavailable: {why}"));
            }
            CallEvent::MicUnavailable(why) => {
                active.notice = Some(format!("No microphone: {why}"));
            }
            CallEvent::Error(message) => {
                if active.status == CallStatus::Connecting {
                    active.status = CallStatus::Failed(message);
                } else {
                    active.notice = Some(message);
                }
            }
            CallEvent::Disconnected => {
                if active.status == CallStatus::Connected {
                    // The engine dropped the room on its own: leave the relay side too.
                    let call_id = active.call.call_id.clone();
                    if let Some(ended) = self.my_call.take() {
                        drop_active_call_images(ended, cx);
                    }
                    if !call_id.is_empty() {
                        self.command(ClientCommand::LeaveCall { call_id });
                    }
                }
            }
        }
        self.sync_tones();
        cx.notify();
    }

    /// The decoded GitHub avatar for `url` as a PNG on disk (notification attachment).
    fn avatar_file_for(&self, url: &str) -> Option<PathBuf> {
        if url.is_empty() {
            return None;
        }
        self.person_notification_png(url)
    }

    /// A sender's initials disc for a notification banner (no photo case).
    fn sender_png_for(&self, sender: &Sender) -> Option<PathBuf> {
        crate::ui::avatars::notification_png(
            &sender.id,
            &sender.display_name(),
            sender.avatar_color_index,
            crate::ui::theme::system_is_dark(),
        )
    }
}

/// FNV-1a 64 for cache keys.
pub fn fnv1a64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

pub fn title_from(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    let mut out: String = line.chars().take(34).collect();
    if line.chars().count() > 34 {
        out.push('…');
    }
    if out.is_empty() {
        "New chat".into()
    } else {
        out
    }
}

/// MIME from a local path's extension (optimistic echo before the relay replies).
fn mime_for_path(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "pdf" => "application/pdf",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "md" | "txt" => "text/plain",
        _ => "application/octet-stream",
    }
}

/// PNG/JPEG/GIF/WebP/ICO → BGRA frame(s) for GPUI (`RenderImage` is BGRA).
/// GIF and animated WebP keep every frame (capped); other formats stay one frame.
pub fn decode_image(bytes: &[u8]) -> Option<RenderImage> {
    match decode_pixels(bytes)? {
        DecodedPixels::Animated(frames) => bgra_frames(frames),
        DecodedPixels::Still(rgba) => bgra_frame(rgba),
    }
}

/// Decode `bytes` for a small tile: the longer side shrinks to `max_side`
/// device pixels (never enlarged). The composer's staged thumbnails use it so
/// a pasted Retina screenshot does not sit in GPU memory at full size.
/// Animated GIF/WebP use the first frame only (the tile does not animate).
pub fn decode_thumbnail(bytes: &[u8], max_side: u32) -> Option<RenderImage> {
    let img = decode_rgba(bytes)?;
    // A macOS window capture (⌘⇧4 + space, "Copy") is the window plus its
    // drop shadow on transparent padding; drawn Cover into a square tile the
    // shadow reads as a second, darker tile behind the picture. The tile shows
    // the opaque content only ([feedback composer-attachment-removal-2026-10-27]).
    let img = match opaque_rect(&img) {
        Some((x, y, w, h)) => image::imageops::crop_imm(&img, x, y, w, h).to_image(),
        None => img,
    };
    let (w, h) = img.dimensions();
    let long = w.max(h);
    let img = if long <= max_side || long == 0 {
        img
    } else {
        let scale = max_side as f32 / long as f32;
        let nw = ((w as f32) * scale).round().max(1.0) as u32;
        let nh = ((h as f32) * scale).round().max(1.0) as u32;
        image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle)
    };
    bgra_frame(img)
}

/// Where the opaque content of `img` sits: the bounding box of pixels with
/// alpha ≥ `OPAQUE_ALPHA` (a drop shadow is fainter than that), or `None`
/// when the picture has no transparent margin worth cropping.
pub fn opaque_rect(img: &image::RgbaImage) -> Option<(u32, u32, u32, u32)> {
    const OPAQUE_ALPHA: u8 = 128;
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
    for (x, y, p) in img.enumerate_pixels() {
        if p.0[3] >= OPAQUE_ALPHA {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    if x0 > x1 || y0 > y1 {
        return None;
    }
    let (cw, ch) = (x1 - x0 + 1, y1 - y0 + 1);
    if (cw, ch) == (w, h) {
        return None;
    }
    Some((x0, y0, cw, ch))
}

/// Longest side kept for chat-bubble (and lightbox) bitmaps.
/// Bubbles paint at most ~360×420 pt; 2× covers Retina. Caps GPU memory so
/// scrolling a chat full of screenshots stays smooth (feedback
/// app-01M5MXPJZARN5QMCE3EZ3EMSGW).
pub const CHAT_IMAGE_MAX_SIDE: u32 = 840;

/// Shrink `img` so its longer side is at most `CHAT_IMAGE_MAX_SIDE`.
pub fn downscale_for_chat(img: image::RgbaImage) -> image::RgbaImage {
    let (w, h) = img.dimensions();
    let long = w.max(h);
    if long <= CHAT_IMAGE_MAX_SIDE {
        return img;
    }
    let scale = CHAT_IMAGE_MAX_SIDE as f32 / long as f32;
    let nw = ((w as f32) * scale).round().max(1.0) as u32;
    let nh = ((h as f32) * scale).round().max(1.0) as u32;
    image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle)
}

/// The picture, plus a copy with its uniform border trimmed when there is one
/// (feedback chat-preview-cards-2026-10-23: a screenshot's empty page margin
/// must not become a white band in the bubble). The viewer shows the untrimmed
/// picture. Both are capped to [`CHAT_IMAGE_MAX_SIDE`] so the transcript can
/// scroll without holding full-resolution textures.
/// Animated GIF/WebP skip the trim (every frame must stay aligned) and return
/// a multi-frame [`RenderImage`].
pub fn decode_image_pair(bytes: &[u8]) -> Option<(RenderImage, Option<RenderImage>)> {
    match decode_pixels(bytes)? {
        DecodedPixels::Animated(frames) => {
            let full = bgra_frames(frames)?;
            Some((full, None))
        }
        DecodedPixels::Still(rgba) => {
            let trimmed = trim_rect(&rgba)
                .map(|(x, y, w, h)| image::imageops::crop_imm(&rgba, x, y, w, h).to_image())
                .map(downscale_for_chat);
            let full = bgra_frame(downscale_for_chat(rgba))?;
            Some((full, trimmed.and_then(bgra_frame)))
        }
    }
}

/// Where the content of `img` sits once a uniform border is removed:
/// `(x, y, w, h)`, or `None` when there is no border worth trimming. The
/// border colour is the corner pixel; a row / column counts as border when
/// every sampled pixel is within 10/255 of it. A 2 % breathing margin is kept,
/// and the trim only happens when it removes at least 6 % of the area and
/// leaves at least a quarter of each side — a plain colour swatch stays whole.
pub fn trim_rect(img: &image::RgbaImage) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = img.dimensions();
    if w < 32 || h < 32 {
        return None;
    }
    let bg = *img.get_pixel(0, 0);
    let near = |p: &image::Rgba<u8>| {
        p.0.iter()
            .zip(bg.0.iter())
            .take(3)
            .all(|(a, b)| a.abs_diff(*b) <= 10)
    };
    let step_x = (w / 160).max(1);
    let step_y = (h / 160).max(1);
    let row_is_bg = |y: u32| {
        (0..w)
            .step_by(step_x as usize)
            .all(|x| near(img.get_pixel(x, y)))
    };
    let col_is_bg = |x: u32| {
        (0..h)
            .step_by(step_y as usize)
            .all(|y| near(img.get_pixel(x, y)))
    };
    let mut top = 0;
    while top < h && row_is_bg(top) {
        top += 1;
    }
    if top >= h {
        return None;
    }
    let mut bottom = h;
    while bottom > top && row_is_bg(bottom - 1) {
        bottom -= 1;
    }
    let mut left = 0;
    while left < w && col_is_bg(left) {
        left += 1;
    }
    let mut right = w;
    while right > left && col_is_bg(right - 1) {
        right -= 1;
    }
    let (pad_x, pad_y) = ((w as f32 * 0.02) as u32, (h as f32 * 0.02) as u32);
    let left = left.saturating_sub(pad_x);
    let top = top.saturating_sub(pad_y);
    let right = (right + pad_x).min(w);
    let bottom = (bottom + pad_y).min(h);
    let (cw, ch) = (right - left, bottom - top);
    if cw < w / 4 || ch < h / 4 {
        return None;
    }
    let kept = (cw as f64 * ch as f64) / (w as f64 * h as f64);
    if kept > 0.94 {
        return None;
    }
    Some((left, top, cw, ch))
}

/// Cap on GIF/WebP frames kept for a chat bubble (feedback
/// app-01M5TGH18WV3QCK5KZT6XKG7FG / app-01M61EZD0GJ2XGV1EW4SCVXENK).
/// Past this, only the first frame is shown.
pub const ANIM_MAX_FRAMES: usize = 300;
/// Cap on decoded RGBA bytes across all animation frames. Past this at the
/// side floor, only the first frame is shown.
pub const ANIM_MAX_DECODED_BYTES: usize = 48 * 1024 * 1024;
/// Starting longer side for animation frames (never upscale past the canvas).
pub const ANIM_SIDE_START: u32 = 480;
/// Smallest longer side tried before falling back to the first frame.
pub const ANIM_SIDE_FLOOR: u32 = 200;

/// Target longer side for animation frames, or `None` → show the first frame
/// only. Starts at [`ANIM_SIDE_START`] (capped to the canvas, never upscaled);
/// shrinks until `frame_count × scaled_w × scaled_h × 4` fits in
/// [`ANIM_MAX_DECODED_BYTES`]; floor [`ANIM_SIDE_FLOOR`]. More than
/// [`ANIM_MAX_FRAMES`], or not fitting at the floor, means first-frame-only.
/// At 200 px, 300 square frames always fit (feedback
/// app-01M61EZD0GJ2XGV1EW4SCVXENK).
pub fn animation_frame_side(frame_count: usize, width: u32, height: u32) -> Option<u32> {
    if frame_count == 0 || frame_count > ANIM_MAX_FRAMES {
        return None;
    }
    let width = width.max(1);
    let height = height.max(1);
    let long = width.max(height);
    let start = ANIM_SIDE_START.min(long);
    if anim_decoded_bytes(frame_count, width, height, ANIM_SIDE_FLOOR) > ANIM_MAX_DECODED_BYTES {
        return None;
    }
    // Highest side in [FLOOR, start] that fits the budget.
    let mut lo = ANIM_SIDE_FLOOR;
    let mut hi = start;
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if anim_decoded_bytes(frame_count, width, height, mid) <= ANIM_MAX_DECODED_BYTES {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    Some(lo)
}

/// RGBA byte cost of `frame_count` frames scaled so the longer side is `side`.
fn anim_decoded_bytes(frame_count: usize, width: u32, height: u32, side: u32) -> usize {
    let long = width.max(height).max(1);
    let scale = side as f64 / long as f64;
    let sw = ((width as f64) * scale).round().max(1.0) as usize;
    let sh = ((height as f64) * scale).round().max(1.0) as usize;
    frame_count
        .saturating_mul(sw)
        .saturating_mul(sh)
        .saturating_mul(4)
}

/// Shrink `img` so its longer side is at most `side` (never enlarged).
fn downscale_to_side(img: image::RgbaImage, side: u32) -> image::RgbaImage {
    let (w, h) = img.dimensions();
    let long = w.max(h);
    if side == 0 || long <= side {
        return img;
    }
    let scale = side as f32 / long as f32;
    let nw = ((w as f32) * scale).round().max(1.0) as u32;
    let nh = ((h as f32) * scale).round().max(1.0) as u32;
    image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle)
}

/// Re-scale already-decoded animation frames down to `side`.
fn rescale_anim_frames(frames: Vec<image::Frame>, side: u32) -> Vec<image::Frame> {
    frames
        .into_iter()
        .map(|frame| {
            let delay = frame.delay();
            let left = frame.left();
            let top = frame.top();
            image::Frame::from_parts(
                downscale_to_side(frame.into_buffer(), side),
                left,
                top,
                delay,
            )
        })
        .collect()
}

fn frame_rgba_bytes(frame: &image::Frame) -> usize {
    let (w, h) = frame.buffer().dimensions();
    (w as usize).saturating_mul(h as usize).saturating_mul(4)
}

/// `page_url` is safe to open from a "via <provider>" caption: only `https`.
pub fn is_https_page_url(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("https") {
        return false;
    }
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    !host.is_empty()
}

/// Still pixels, or every frame of an animation (already scaled, RGBA, with delays).
enum DecodedPixels {
    Still(image::RgbaImage),
    Animated(Vec<image::Frame>),
}

/// GIF87a / GIF89a magic.
fn looks_like_gif(bytes: &[u8]) -> bool {
    bytes.starts_with(b"GIF8")
}

/// RIFF....WEBP magic.
fn looks_like_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP"
}

/// PNG/JPEG/GIF/WebP/ICO → RGBA pixels (first / only frame). Used by thumbnails
/// and the still path for formats that are not GIF/WebP.
fn decode_rgba(bytes: &[u8]) -> Option<image::RgbaImage> {
    Some(image::load_from_memory(bytes).ok()?.into_rgba8())
}

/// Decode `bytes`: multi-frame for GIF/animated WebP, still otherwise.
fn decode_pixels(bytes: &[u8]) -> Option<DecodedPixels> {
    if looks_like_gif(bytes) || looks_like_webp(bytes) {
        // A still WebP (or a GIF the animation decoder rejects) falls back to
        // the single-frame path so the bubble still shows something.
        return decode_animation_pixels(bytes)
            .or_else(|| decode_rgba(bytes).map(DecodedPixels::Still));
    }
    Some(DecodedPixels::Still(decode_rgba(bytes)?))
}

/// All frames of a GIF or WebP, each scaled to the animation side helper.
/// A still WebP (or a single-frame GIF) becomes [`DecodedPixels::Still`].
/// Frame count is unknown up front: decode one frame at a time at the current
/// side; when the running total would pass the budget at frame `n`, ask the
/// helper with `2n` frames, rescale held frames, and continue — or fall back
/// to the first frame when the helper says so (feedback
/// app-01M61EZD0GJ2XGV1EW4SCVXENK).
fn decode_animation_pixels(bytes: &[u8]) -> Option<DecodedPixels> {
    use image::AnimationDecoder;
    use std::io::Cursor;

    let frames_iter: Box<dyn Iterator<Item = image::ImageResult<image::Frame>>> =
        if looks_like_gif(bytes) {
            let dec = image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).ok()?;
            Box::new(dec.into_frames())
        } else {
            let dec = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes)).ok()?;
            Box::new(dec.into_frames())
        };

    let mut frames: Vec<image::Frame> = Vec::new();
    let mut decoded_bytes: usize = 0;
    let mut side = ANIM_SIDE_START;
    let mut canvas: Option<(u32, u32)> = None;
    for result in frames_iter {
        let frame = result.ok()?;
        let delay = frame.delay();
        let left = frame.left();
        let top = frame.top();
        let buffer = frame.into_buffer();
        let (cw, ch) = buffer.dimensions();
        if canvas.is_none() {
            canvas = Some((cw.max(1), ch.max(1)));
            side = ANIM_SIDE_START.min(cw.max(ch).max(1));
        }
        if frames.len() >= ANIM_MAX_FRAMES {
            return frames
                .into_iter()
                .next()
                .map(|f| DecodedPixels::Still(f.into_buffer()));
        }
        let mut scaled = downscale_to_side(buffer, side);
        let mut frame_bytes = {
            let (w, h) = scaled.dimensions();
            (w as usize).saturating_mul(h as usize).saturating_mul(4)
        };
        if decoded_bytes.saturating_add(frame_bytes) > ANIM_MAX_DECODED_BYTES {
            // Running total would pass the budget at frame n (1-based).
            let n = frames.len() + 1;
            let (cw, ch) = canvas.expect("canvas set on first frame");
            match animation_frame_side(n.saturating_mul(2), cw, ch) {
                None => {
                    return frames
                        .into_iter()
                        .next()
                        .map(|f| DecodedPixels::Still(f.into_buffer()))
                        .or(Some(DecodedPixels::Still(scaled)));
                }
                Some(new_side) => {
                    if new_side < side {
                        frames = rescale_anim_frames(frames, new_side);
                        decoded_bytes = frames.iter().map(frame_rgba_bytes).sum();
                        side = new_side;
                        scaled = downscale_to_side(scaled, side);
                        let (w, h) = scaled.dimensions();
                        frame_bytes = (w as usize).saturating_mul(h as usize).saturating_mul(4);
                    }
                    if decoded_bytes.saturating_add(frame_bytes) > ANIM_MAX_DECODED_BYTES {
                        return frames
                            .into_iter()
                            .next()
                            .map(|f| DecodedPixels::Still(f.into_buffer()))
                            .or(Some(DecodedPixels::Still(scaled)));
                    }
                }
            }
        }
        decoded_bytes += frame_bytes;
        frames.push(image::Frame::from_parts(scaled, left, top, delay));
    }
    match frames.len() {
        0 => None,
        1 => Some(DecodedPixels::Still(
            frames.into_iter().next()?.into_buffer(),
        )),
        _ => Some(DecodedPixels::Animated(frames)),
    }
}

/// RGBA pixels → BGRA (swap R/B in place).
fn rgba_to_bgra(img: image::RgbaImage) -> Option<image::RgbaImage> {
    let (w, h) = img.dimensions();
    let mut raw = img.into_raw();
    for px in raw.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    image::RgbaImage::from_raw(w, h, raw)
}

/// RGBA pixels → the BGRA frame GPUI draws.
fn bgra_frame(img: image::RgbaImage) -> Option<RenderImage> {
    let buffer = rgba_to_bgra(img)?;
    Some(RenderImage::new(vec![image::Frame::new(buffer)]))
}

/// Many RGBA frames → one multi-frame BGRA [`RenderImage`] (delays kept).
fn bgra_frames(frames: Vec<image::Frame>) -> Option<RenderImage> {
    let mut out = Vec::with_capacity(frames.len());
    for frame in frames {
        let delay = frame.delay();
        let left = frame.left();
        let top = frame.top();
        let buffer = rgba_to_bgra(frame.into_buffer())?;
        out.push(image::Frame::from_parts(buffer, left, top, delay));
    }
    Some(RenderImage::new(out))
}

fn same_shape(a: &Message, b: &Message) -> bool {
    a.parts
        .iter()
        .zip(b.parts.iter())
        .all(|(x, y)| match (x, y) {
            (Part::Text { id: xa, .. }, Part::Text { id: ya, .. }) => xa == ya,
            (Part::Reasoning { id: xa, .. }, Part::Reasoning { id: ya, .. }) => xa == ya,
            (Part::Text { .. }, _) | (Part::Reasoning { .. }, _) => false,
            (other_a, other_b) => other_a == other_b,
        })
}

/// The message is waiting on the user: an open `question` part or the legacy
/// `waiting_user` status.
fn needs_input(m: &Message) -> bool {
    m.open_questions().next().is_some()
        || m.parts.iter().any(|p| {
            matches!(
                p,
                Part::Status {
                    code: StatusKind::AskUser,
                    ..
                }
            )
        })
}

/// First non-empty text line, clipped for a notification body.
fn preview(m: &Message) -> String {
    const MAX: usize = 96;
    let line = m
        .parts
        .iter()
        .find_map(|p| match p {
            Part::Text { text, .. } => text.lines().map(str::trim).find(|l| !l.is_empty()),
            _ => None,
        })
        .unwrap_or("");
    if line.chars().count() <= MAX {
        line.to_string()
    } else {
        let cut: String = line.chars().take(MAX - 1).collect();
        format!("{cut}…")
    }
}

/// What a message that just landed does to the chat's read state (#1438).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArrivalRead {
    /// Someone else's content in a chat not on screen in a focused window: light the dot.
    MarkUnread,
    /// Someone else's content in the chat the user is looking at: the user has seen it, so
    /// the read cursor advances (on the relay, for every device) and nothing lights.
    AdvanceCursor,
    /// Own message (from any device) or a system line: never unread, nothing to advance.
    Nothing,
}

/// Decide the read effect of an arrival. `message_own` = sent by this user (any device);
/// `system_line` = a relay/system notice such as a membership change.
pub(crate) fn arrival_read(
    chat_visible: bool,
    message_own: bool,
    system_line: bool,
) -> ArrivalRead {
    if message_own || system_line {
        ArrivalRead::Nothing
    } else if chat_visible {
        ArrivalRead::AdvanceCursor
    } else {
        ArrivalRead::MarkUnread
    }
}

pub fn init_source(cx: &mut App) -> Box<dyn SyncSource> {
    let _ = cx;
    crate::sync::source_from_env()
}

/// What the relay lists this device as (`device_name` on sign-in).
fn device_name() -> String {
    let host = std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "Mac".into());
    let host = host.trim_end_matches(".local");
    if cfg!(target_os = "macos") {
        format!("{host} (Mac)")
    } else {
        host.to_string()
    }
}

/// The one rule behind ⌘T, the header pill's "+" and the tab dropdown's New
/// tab: only an agent's own space takes a new tab. People DMs and groups
/// (`SpaceKind::Dm` / `Group`) have no project chats, a server channel has no
/// space at all, and the whole thing sits behind the multi-chat flag.
pub(crate) fn new_tab_allowed(
    multi_chat: bool,
    space: Option<&Project>,
    channel_open: bool,
) -> bool {
    if !multi_chat || channel_open {
        return false;
    }
    space.is_some_and(|p| !p.is_people())
}

/// The local row of a session fork (#1257): the same project, a plain chat
/// titled `<title> (fork)`, nothing pinned or shared. `None` when `source`
/// cannot be forked (groups, system tabs, chats of shared or people spaces).
pub(crate) fn forked_chat(source: &Chat, project: &Project, id: Id) -> Option<Chat> {
    if project.shared || project.is_people() {
        return None;
    }
    if source.is_group() || source.is_system || source.kind != ChatKind::Chat {
        return None;
    }
    if source.project_id != project.id {
        return None;
    }
    Some(Chat {
        id,
        project_id: project.id.clone(),
        title: fork_title(&crate::ui::header::tab_title(source, project)),
        is_pinned: false,
        is_system: false,
        kind: ChatKind::Chat,
        participants: Vec::new(),
        unread: false,
        unread_count: 0,
        last_message_at: now_ms(),
        archived_at: None,
    })
}

/// `"unicorns"` → `"unicorns (fork)"`; a fork of a fork stays `"… (fork)"`.
/// Mirrors the relay's default so the row the Mac shows first is the one the
/// relay returns.
pub(crate) fn fork_title(source: &str) -> String {
    let base = source.trim();
    if base.is_empty() {
        return "New chat (fork)".into();
    }
    if base.ends_with("(fork)") {
        return base.to_owned();
    }
    format!("{base} (fork)")
}

/// An empty (or bare `@`) people query asks for the whole directory.
pub(crate) fn is_directory_query(query: &str) -> bool {
    query.trim().trim_start_matches('@').is_empty()
}

/// What the New Message sheet shows on its first frame: the known directory, and
/// "Searching…" only when nothing is known yet (first open after sign-in, offline).
pub(crate) fn sheet_first_paint(directory: &[Person]) -> (Vec<Person>, bool) {
    (directory.to_vec(), directory.is_empty())
}

/// The list to show after a refresh — `None` when it equals what is on screen,
/// so a refetch behind an open sheet never repaints an unchanged list.
pub(crate) fn directory_update(current: &[Person], fresh: Vec<Person>) -> Option<Vec<Person>> {
    (fresh != current).then_some(fresh)
}

/// What the people sheet (New message, Add members, Give a person access…)
/// lists for `query`: the known directory plus the relay's answer, one row per
/// handle, through the shared matcher on display name and @handle — exact >
/// prefix > word-start, name before handle, directory order inside a tie
/// ([feedback shared-search-matcher-mac-2026-10-26]: "aff" lists Affine and
/// AffineTeam next to Andreas F @aff, not @aff alone). An empty query keeps
/// everyone in directory order.
pub(crate) fn people_sheet_results(
    directory: &[Person],
    relay: &[Person],
    query: &str,
) -> Vec<Person> {
    use arbos_core::search::{filter, normalize_query, Fields};
    let mut seen: HashSet<String> = HashSet::new();
    let mut pool: Vec<Person> = Vec::with_capacity(directory.len() + relay.len());
    for p in directory.iter().chain(relay.iter()) {
        if seen.insert(p.handle.to_lowercase()) {
            pool.push(p.clone());
        }
    }
    filter(pool, &normalize_query(query), |p| {
        Fields::new(p.display_name(), p.handle.clone())
    })
}

#[cfg(test)]
mod unread_arrival_tests {
    use super::{arrival_read, ArrivalRead};

    #[test]
    fn someone_elses_message_in_hidden_chat_marks_unread() {
        assert_eq!(arrival_read(false, false, false), ArrivalRead::MarkUnread);
    }

    /// #1438: own message (any device) → nothing; other's message while the chat is on screen
    /// → the cursor advances; other's message in a hidden chat / inactive window → unread;
    /// a system line never counts.
    #[test]
    fn arrival_read_decides_cursor_versus_dot() {
        assert_eq!(arrival_read(true, true, false), ArrivalRead::Nothing);
        assert_eq!(arrival_read(false, true, false), ArrivalRead::Nothing);
        assert_eq!(arrival_read(true, false, false), ArrivalRead::AdvanceCursor);
        assert_eq!(arrival_read(false, false, false), ArrivalRead::MarkUnread);
        assert_eq!(arrival_read(false, false, true), ArrivalRead::Nothing);
        assert_eq!(arrival_read(true, false, true), ArrivalRead::Nothing);
    }
}

#[cfg(test)]
mod hidden_chat_open_tests {
    //! Scout (Arbonites, 2026-10-29 16:49 UTC): "it also unarchives the chat if you open it"
    //! ([feedback archived-chat-stays-archived-2026-10-29]). Opening a row from Hidden chats —
    //! or from a search hit — only views it; only an incoming message from someone else while
    //! the user looks elsewhere brings it back (Telegram), and Unarchive does.
    use std::sync::{Arc, Mutex};

    use futures::future::BoxFuture;
    use gpui::{AppContext as _, Entity, TestAppContext};
    use tokio::sync::mpsc;

    use super::*;
    use crate::model::{AccountView, ChatKind, SpaceKind};

    type Commands = Arc<Mutex<Option<mpsc::UnboundedReceiver<ClientCommand>>>>;

    struct Recording(Commands);

    impl SyncSource for Recording {
        fn name(&self) -> &'static str {
            "recording"
        }

        fn run(
            self: Box<Self>,
            _events: mpsc::UnboundedSender<SyncEvent>,
            commands: mpsc::UnboundedReceiver<ClientCommand>,
        ) -> BoxFuture<'static, anyhow::Result<()>> {
            *self.0.lock().expect("commands slot") = Some(commands);
            Box::pin(std::future::pending())
        }
    }

    fn chat(id: &str, project_id: &str) -> Chat {
        Chat {
            id: id.into(),
            project_id: project_id.into(),
            title: String::new(),
            is_pinned: true,
            is_system: false,
            unread: false,
            unread_count: 0,
            last_message_at: 1_000,
            archived_at: None,
            kind: ChatKind::Chat,
            participants: Vec::new(),
        }
    }

    fn message(id: &str, chat_id: &str, sender: &str, text: &str, at: u64) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id, "chatId": chat_id, "role": "user", "sender": sender, "status": "done",
            "createdAt": at, "completedAt": at,
            "parts": [{ "id": format!("{id}-t"), "kind": "text", "text": text }],
        }))
        .expect("message")
    }

    /// Agent `arbos` open; agent `lium` and DM `dm` (with Sam) both in Hidden chats.
    fn mount(cx: &mut TestAppContext) -> (tokio::runtime::Runtime, Commands, Entity<AppState>) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let handle = rt.handle().clone();
        cx.update(|cx| gpui_tokio::init_from_handle(cx, handle));
        let commands: Commands = Arc::new(Mutex::new(None));
        let state = cx.new(|cx| AppState::new(Box::new(Recording(commands.clone())), cx));
        cx.run_until_parked();
        for _ in 0..3 {
            rt.block_on(tokio::task::yield_now());
        }
        state.update(cx, |s, cx| {
            s.account = Some(AccountView {
                id: "gh_1".into(),
                login: "scout".into(),
                name: "Scout".into(),
                avatar_url: String::new(),
                device_name: "test".into(),
                balance_usd: 0.0,
                spent_today_usd: 0.0,
                daily_cap_usd: None,
            });
            for (id, kind) in [
                ("arbos", SpaceKind::Project),
                ("lium", SpaceKind::Project),
                ("dm", SpaceKind::Dm),
            ] {
                s.projects.push(Project {
                    id: id.into(),
                    name: id.into(),
                    kind,
                    people: if kind == SpaceKind::Dm {
                        vec![Person {
                            id: "gh_2".into(),
                            handle: "sam".into(),
                            ..Person::default()
                        }]
                    } else {
                        Vec::new()
                    },
                    ..Project::default()
                });
            }
            s.chats
                .insert("arbos".into(), vec![chat("arbos-main", "arbos")]);
            s.chats
                .insert("lium".into(), vec![chat("lium-main", "lium")]);
            s.chats.insert("dm".into(), vec![chat("dm", "dm")]);
            s.select_project("arbos".into(), cx);
            s.window_active = true;
            s.set_space_archived("lium".into(), true, cx);
            s.set_space_archived("dm".into(), true, cx);
            // What the next registry refresh adds for an archived people chat.
            s.archived_chats.insert("dm".into());
        });
        let _ = unarchives(&commands);
        (rt, commands, state)
    }

    /// Every unarchive the state sent to the relay since the last call.
    fn unarchives(commands: &Commands) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(rx) = commands.lock().expect("commands").as_mut() {
            while let Ok(c) = rx.try_recv() {
                match c {
                    ClientCommand::ArchiveChat {
                        chat_id,
                        archived: false,
                    } => out.push(chat_id),
                    ClientCommand::ArchiveSpace {
                        space_id,
                        archived: false,
                        ..
                    } => out.push(space_id),
                    _ => {}
                }
            }
        }
        out
    }

    fn hidden(state: &Entity<AppState>, cx: &mut TestAppContext) -> (bool, bool, usize) {
        state.read_with(cx, |s, _| {
            (
                s.is_space_archived("lium"),
                s.is_space_archived("dm"),
                s.archived_space_count(),
            )
        })
    }

    #[gpui::test]
    fn opening_a_hidden_agent_or_dm_only_views_it(cx: &mut TestAppContext) {
        let (_rt, commands, state) = mount(cx);
        assert_eq!(hidden(&state, cx), (true, true, 2));
        state.update(cx, |s, cx| s.select_project("dm".into(), cx));
        assert_eq!(
            state.read_with(cx, |s, _| s.active_chat_id()),
            Some("dm".into())
        );
        state.update(cx, |s, cx| s.select_project("lium".into(), cx));
        assert_eq!(
            state.read_with(cx, |s, _| s.active_chat_id()),
            Some("lium-main".into())
        );
        assert_eq!(
            hidden(&state, cx),
            (true, true, 2),
            "both stay in Hidden chats"
        );
        assert!(
            unarchives(&commands).is_empty(),
            "nothing asked the relay to unarchive"
        );
    }

    #[gpui::test]
    fn a_message_arriving_in_the_open_hidden_chat_keeps_it_hidden(cx: &mut TestAppContext) {
        let (_rt, commands, state) = mount(cx);
        state.update(cx, |s, cx| {
            s.select_project("dm".into(), cx);
            s.apply(
                SyncEvent::MessageUpsert {
                    chat_id: "dm".into(),
                    message: Box::new(message("m1", "dm", "user:gh_2", "you there?", now_ms())),
                },
                cx,
            );
        });
        assert_eq!(
            hidden(&state, cx),
            (true, true, 2),
            "seen on screen: still hidden"
        );
        assert!(unarchives(&commands).is_empty());
    }

    #[gpui::test]
    fn a_search_hit_in_a_hidden_dm_opens_it_and_leaves_it_hidden(cx: &mut TestAppContext) {
        let (_rt, commands, state) = mount(cx);
        state.update(cx, |s, cx| {
            s.messages.insert(
                "dm".into(),
                vec![message("m1", "dm", "user:gh_2", "pizza tonight", 1_000)],
            );
            s.index_chat("dm");
            s.open_from_home_search("dm".into(), "pizza", cx);
        });
        assert_eq!(
            state.read_with(cx, |s, _| s.highlight_message.clone()),
            Some("m1".into()),
            "the hit was revealed"
        );
        assert_eq!(
            state.read_with(cx, |s, _| s.active_chat_id()),
            Some("dm".into())
        );
        assert_eq!(hidden(&state, cx), (true, true, 2));
        assert!(state.read_with(cx, |s, _| s.archived_chats.contains("dm")));
        assert!(unarchives(&commands).is_empty());
    }

    #[gpui::test]
    fn someone_writing_while_i_look_elsewhere_brings_the_row_back(cx: &mut TestAppContext) {
        let (_rt, commands, state) = mount(cx);
        state.update(cx, |s, cx| {
            s.apply(
                SyncEvent::MessageUpsert {
                    chat_id: "dm".into(),
                    message: Box::new(message("m1", "dm", "user:gh_2", "you there?", now_ms())),
                },
                cx,
            );
        });
        assert_eq!(
            hidden(&state, cx),
            (true, false, 1),
            "Telegram: new activity unhides"
        );
        assert_eq!(unarchives(&commands), vec!["dm".to_string()]);
    }

    #[gpui::test]
    fn another_device_archiving_a_dm_hides_its_row_here(cx: &mut TestAppContext) {
        let (_rt, _commands, state) = mount(cx);
        state.update(cx, |s, cx| {
            s.apply(
                SyncEvent::ChatArchived {
                    chat_id: "dm".into(),
                    archived_at: None,
                },
                cx,
            );
        });
        assert_eq!(
            hidden(&state, cx),
            (true, false, 1),
            "unarchived elsewhere: back"
        );
        state.update(cx, |s, cx| {
            s.apply(
                SyncEvent::ChatArchived {
                    chat_id: "dm".into(),
                    archived_at: Some(5),
                },
                cx,
            );
        });
        assert_eq!(
            hidden(&state, cx),
            (true, true, 2),
            "archived elsewhere: hidden here"
        );
    }
}

/// The block-list row for `person`: same non-empty id, else the same handle (GitHub logins
/// are case-insensitive).
pub(crate) fn blocked_entry<'a>(blocked: &'a [Person], person: &Person) -> Option<&'a Person> {
    blocked.iter().find(|b| {
        (!b.id.is_empty() && b.id == person.id)
            || (!b.handle.is_empty() && b.handle.eq_ignore_ascii_case(&person.handle))
    })
}

#[cfg(test)]
mod block_state_tests {
    //! #1545 (Jacob, 2026-10-23): after "Block user" the row kept reading Block user. The
    //! row now derives from the relay's block list (`SyncEvent::Blocked`), so it flips to
    //! Unblock once the list names the person and back once it no longer does.
    use super::blocked_entry;
    use crate::model::Person;
    use crate::ui::profiles::user::block_row_label;

    fn person(id: &str, handle: &str) -> Person {
        Person {
            id: id.into(),
            handle: handle.into(),
            ..Default::default()
        }
    }

    #[test]
    fn blocked_matches_by_id_then_handle() {
        let ken = person("gh_1", "kenobijon");
        let list = vec![ken.clone()];
        assert!(blocked_entry(&list, &ken).is_some());
        // The pane may open from a row that only knows the handle.
        assert!(blocked_entry(&list, &person("", "KenobiJon")).is_some());
        // A different id with a different handle is not blocked.
        assert!(blocked_entry(&list, &person("gh_2", "sami")).is_none());
        assert!(blocked_entry(&[], &ken).is_none());
    }

    #[test]
    fn block_row_flips_with_the_list() {
        let ken = person("gh_1", "kenobijon");
        let mut list: Vec<Person> = Vec::new();
        assert_eq!(
            block_row_label(blocked_entry(&list, &ken).is_some()),
            "Block user"
        );
        list.push(ken.clone());
        assert_eq!(
            block_row_label(blocked_entry(&list, &ken).is_some()),
            "Unblock"
        );
        list.retain(|p| p.id != ken.id);
        assert_eq!(
            block_row_label(blocked_entry(&list, &ken).is_some()),
            "Block user"
        );
    }
}

#[cfg(test)]
mod new_message_first_paint_tests {
    //! Jacob, iPhone, 2026-10-22: the New Message sheet "loads weird" — it opened
    //! on a spinner / "Searching…" and the people list arrived after it. The sheet
    //! now opens on the directory it already has. Mirrors the iPhone's
    //! `NewMessageSheetTests`.
    use super::*;

    fn person(id: &str, handle: &str) -> Person {
        Person {
            id: id.into(),
            handle: handle.into(),
            name: Some(handle.to_uppercase()),
            ..Default::default()
        }
    }

    fn directory() -> Vec<Person> {
        vec![
            person("gh_1", "allan"),
            person("gh_2", "alw-one"),
            person("test_3", "arbosdemo-alice"),
        ]
    }

    #[test]
    fn first_frame_is_the_known_directory_not_searching() {
        let (rows, busy) = sheet_first_paint(&directory());
        assert_eq!(rows, directory());
        assert!(!busy, "no Searching… row over a known list");
    }

    #[test]
    fn only_an_unknown_directory_says_searching() {
        let (rows, busy) = sheet_first_paint(&[]);
        assert!(rows.is_empty());
        assert!(busy);
    }

    #[test]
    fn unchanged_refresh_does_not_repaint() {
        assert_eq!(directory_update(&directory(), directory()), None);
        assert_eq!(
            directory_update(&[], directory()),
            Some(directory()),
            "the first answer paints"
        );
        let mut renamed = directory();
        renamed[1].name = Some("Al W.".into());
        assert_eq!(
            directory_update(&directory(), renamed.clone()),
            Some(renamed)
        );
        let shorter = directory()[..2].to_vec();
        assert_eq!(
            directory_update(&directory(), shorter.clone()),
            Some(shorter)
        );
    }

    #[test]
    fn directory_query_is_empty_or_a_bare_at() {
        assert!(is_directory_query(""));
        assert!(is_directory_query("  "));
        assert!(is_directory_query("@"));
        assert!(!is_directory_query("al"));
        assert!(!is_directory_query("@al"));
    }
}

#[cfg(test)]
mod update_restart_chain_tests {
    //! Affinity (Arbonites, 2026-10-28 21:47 UTC): "bubble just caused my arbos to restart".
    //! No chat event did it: her Mac had 0.6.334 installed and waiting since 17:57, relaunched
    //! into it at 21:38, into 0.6.338 at 21:47 and into 0.6.339 at 22:00 — a build that waits
    //! for its relaunch ignored every newer one, so each missed release was one more restart
    //! ([feedback mac-update-restart-chain-2026-10-28]).
    use futures::future::BoxFuture;
    use gpui::{AppContext as _, TestAppContext};
    use tokio::sync::mpsc;

    use super::*;
    use arbos_update::client::{Available, Install};

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

    fn build(version: &str) -> Available {
        Available {
            item: arbos_update::appcast::Item {
                title: format!("Arbos {version}"),
                version: version.into(),
                build: "1".into(),
                ..Default::default()
            },
            url: format!("https://arbos.life/updates/mac/Arbos-{version}-macos-universal.tar.gz"),
            length: 1,
            ed_signature: String::new(),
        }
    }

    fn bundle() -> Install {
        Install::MacBundle("/Applications/Arbos.app".into())
    }

    /// Auto-update on, running 0.6.333, the window in the background (no relaunch yet).
    #[allow(clippy::type_complexity)]
    fn mount(
        cx: &mut TestAppContext,
    ) -> (
        tokio::runtime::Runtime,
        gpui::Entity<AppState>,
        mpsc::UnboundedReceiver<UpdateCommand>,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let handle = rt.handle().clone();
        cx.update(|cx| gpui_tokio::init_from_handle(cx, handle));
        let state = cx.new(|cx| AppState::new(Box::new(Idle), cx));
        let (tx, rx) = mpsc::unbounded_channel();
        state.update(cx, |s, _| {
            s.update_commands = tx;
            s.update.auto = true;
            s.update.needs_relocation = false;
            s.update.current_version = "0.6.333".into();
            s.window_active = false;
        });
        (rt, state, rx)
    }

    fn installs(rx: &mut mpsc::UnboundedReceiver<UpdateCommand>) -> usize {
        std::iter::from_fn(|| rx.try_recv().ok())
            .filter(|c| matches!(c, UpdateCommand::Install))
            .count()
    }

    fn waiting_version(state: &AppState, _: &App) -> Option<String> {
        match &state.update.phase {
            UpdatePhase::Installed { available, .. } => Some(available.item.version.clone()),
            _ => None,
        }
    }

    /// Install `version` through the automatic path the way the worker reports it.
    fn land(cx: &mut TestAppContext, state: &gpui::Entity<AppState>, version: &str) {
        state.update(cx, |s, cx| {
            s.apply_update(UpdateEvent::Installing, cx);
            s.apply_update(UpdateEvent::Installed(bundle()), cx);
        });
        let waiting = state.read_with(cx, waiting_version);
        assert_eq!(
            waiting.as_deref(),
            Some(version),
            "{version} waits for its relaunch"
        );
    }

    #[gpui::test]
    fn a_newer_release_replaces_the_waiting_build_so_there_is_one_restart(cx: &mut TestAppContext) {
        let (_rt, state, mut rx) = mount(cx);
        state.update(cx, |s, cx| {
            s.apply_update(UpdateEvent::Available(Box::new(build("0.6.334"))), cx)
        });
        assert_eq!(installs(&mut rx), 1, "0.6.334 installs in the background");
        land(cx, &state, "0.6.334");

        // Releases that land while 0.6.334 waits are installed over it.
        for newer in ["0.6.338", "0.6.339"] {
            state.update(cx, |s, cx| {
                s.apply_update(UpdateEvent::Available(Box::new(build(newer))), cx)
            });
            assert_eq!(
                installs(&mut rx),
                1,
                "{newer} is installed over the waiting build"
            );
            state.read_with(cx, |s, _| {
                assert!(s.update.busy(), "{newer} is downloading");
                assert!(s.pending_relaunch.is_some(), "the relaunch still waits");
            });
            land(cx, &state, newer);
        }

        // The same or an older build in the feed changes nothing.
        for same in ["0.6.339", "0.6.338"] {
            state.update(cx, |s, cx| {
                s.apply_update(UpdateEvent::Available(Box::new(build(same))), cx)
            });
            assert_eq!(
                installs(&mut rx),
                0,
                "{same} is not newer than the waiting 0.6.339"
            );
            assert_eq!(
                state.read_with(cx, waiting_version).as_deref(),
                Some("0.6.339")
            );
        }
    }

    /// The relaunch never fires while the newer build is being swapped into the bundle.
    #[gpui::test]
    fn no_relaunch_while_the_newer_build_installs(cx: &mut TestAppContext) {
        let (_rt, state, mut rx) = mount(cx);
        state.update(cx, |s, cx| {
            s.apply_update(UpdateEvent::Available(Box::new(build("0.6.334"))), cx)
        });
        land(cx, &state, "0.6.334");
        state.update(cx, |s, cx| {
            s.apply_update(UpdateEvent::Available(Box::new(build("0.6.338"))), cx);
            let long_ago = now_ms().saturating_sub(10 * update::IDLE_RELAUNCH_MS);
            s.window_active = true;
            s.became_active_at = long_ago;
            s.last_input_at = long_ago;
            assert!(!s.relaunch_is_safe(), "downloading 0.6.338");
            s.apply_update(UpdateEvent::Installing, cx);
            assert!(!s.relaunch_is_safe(), "installing 0.6.338");
            assert!(!s.try_relaunch(cx));
            assert!(s.pending_relaunch.is_some());
        });
        assert_eq!(installs(&mut rx), 2);
    }

    /// Manual mode: the waiting build is what the user clicked for; the feed does not move it.
    #[gpui::test]
    fn manual_mode_keeps_the_waiting_build(cx: &mut TestAppContext) {
        let (_rt, state, mut rx) = mount(cx);
        state.update(cx, |s, cx| {
            s.apply_update(UpdateEvent::Available(Box::new(build("0.6.334"))), cx)
        });
        land(cx, &state, "0.6.334");
        let _ = installs(&mut rx);
        state.update(cx, |s, cx| {
            s.update.auto = false;
            s.apply_update(UpdateEvent::Available(Box::new(build("0.6.338"))), cx);
        });
        assert_eq!(installs(&mut rx), 0);
        assert_eq!(
            state.read_with(cx, waiting_version).as_deref(),
            Some("0.6.334")
        );
    }
}

#[cfg(test)]
mod channel_sender_tests {
    //! Affinity (Arbonites, 2026-10-28 16:52 UTC, confirmed by Scout): "bug where we are
    //! referred to as our handles instead of our usernames in server" — a channel bubble
    //! read "A" where a group bubble reads "Scout". The homeserver names a person by handle;
    //! the display name comes from the server's roster ([feedback
    //! server-member-names-2026-10-28]).
    use futures::future::BoxFuture;
    use gpui::{AppContext as _, TestAppContext};
    use tokio::sync::mpsc;

    use super::*;
    use crate::model::{AccountView, ServerInfo};

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

    const ROOM: &str = "!general:arbos.life";
    const SERVER: &str = "01SERVERTEST0000000000000";

    /// A homeserver event by `mxid` through the real row mapper, with the Matrix display
    /// name the relay registers (the handle).
    fn matrix_row(id: &str, mxid: &str, handle: &str, text: &str, at: i64) -> MessageRow {
        let ev = serde_json::json!({
            "type": "m.room.message", "event_id": format!("${id}"), "sender": mxid,
            "origin_server_ts": at,
            "content": { "msgtype": "m.text", "body": text, "life.arbos.id": id },
        });
        arbos_matrix::rows::message_row(&ev, &|_| Some(handle.to_owned())).expect("row")
    }

    fn scout() -> Person {
        Person {
            id: "test_01SCOUT".into(),
            handle: "A".into(),
            name: Some("Scout".into()),
            avatar_url: "https://relay.test/a/scout.png".into(),
            ..Person::default()
        }
    }

    fn server(members: Vec<Person>) -> ServerInfo {
        ServerInfo {
            id: SERVER.into(),
            name: "test server".into(),
            members,
            ..ServerInfo::default()
        }
    }

    fn mount(cx: &mut TestAppContext) -> (tokio::runtime::Runtime, gpui::Entity<AppState>) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let handle = rt.handle().clone();
        cx.update(|cx| gpui_tokio::init_from_handle(cx, handle));
        let state = cx.new(|cx| AppState::new(Box::new(Idle), cx));
        state.update(cx, |s, _| {
            s.account = Some(AccountView {
                id: "test_01ME".into(),
                login: "affinity".into(),
                name: "Affinity".into(),
                avatar_url: String::new(),
                device_name: "test".into(),
                balance_usd: 0.0,
                spent_today_usd: 0.0,
                daily_cap_usd: None,
            });
        });
        (rt, state)
    }

    fn names(state: &AppState) -> Vec<(String, Option<String>, Option<String>)> {
        state
            .channel_rows
            .get(ROOM)
            .map(|rows| {
                rows.iter()
                    .map(|m| {
                        (
                            m.sender_handle.clone().unwrap_or_default(),
                            m.sender_name.clone(),
                            m.sender_avatar.clone(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// With the server's members known, a channel row by Scout (`@A`) is named "Scout" and
    /// gets their picture; my own row keeps my member string; the handle stays the handle.
    #[gpui::test]
    fn a_channel_row_is_named_from_the_server_roster(cx: &mut TestAppContext) {
        let (_rt, state) = mount(cx);
        state.update(cx, |s, cx| {
            s.servers.push(server(vec![scout()]));
            s.on_matrix_timeline(
                ROOM.into(),
                vec![
                    matrix_row("m1", "@test_01scout:arbos.life", "A", "yo", 1_000),
                    matrix_row("m2", "@test_01me:arbos.life", "affinity", "who is A", 2_000),
                ],
                cx,
            );
        });
        let got = state.read_with(cx, |s, _| names(s));
        assert_eq!(
            got,
            vec![
                (
                    "A".into(),
                    Some("Scout".into()),
                    Some("https://relay.test/a/scout.png".into())
                ),
                ("affinity".into(), None, None),
            ],
            "Scout by display name, picture from the roster; mine untouched"
        );
        let senders: Vec<String> = state.read_with(cx, |s, _| {
            s.channel_rows[ROOM]
                .iter()
                .map(|m| m.sender.clone())
                .collect()
        });
        assert_eq!(
            senders,
            vec![
                "user:test_01SCOUT".to_string(),
                "user:test_01ME".to_string()
            ],
            "member strings take the relay's spelling of the id (one disc colour per person)"
        );
        // The bubble's name label comes from `Person::display_name()` of the row's sender.
        let shown = state.read_with(cx, |s, _| {
            let m = &s.channel_rows[ROOM][0];
            Person {
                handle: m.sender_handle.clone().unwrap_or_default(),
                name: m.sender_name.clone(),
                ..Person::default()
            }
            .display_name()
        });
        assert_eq!(shown, "Scout");
    }

    /// The roster can come after the rows (a channel opened before `GET /servers/{id}`
    /// answered): the rows are named when it lands, and never read the handle as a name
    /// in between. The list asks for every server's roster once.
    #[gpui::test]
    fn rows_are_renamed_when_the_roster_arrives(cx: &mut TestAppContext) {
        let (_rt, state) = mount(cx);
        state.update(cx, |s, cx| {
            s.on_servers(vec![server(vec![])], cx);
            s.on_matrix_timeline(
                ROOM.into(),
                vec![matrix_row(
                    "m1",
                    "@test_01scout:arbos.life",
                    "A",
                    "yo",
                    1_000,
                )],
                cx,
            );
        });
        assert_eq!(
            state.read_with(cx, |s, _| names(s)),
            vec![("A".into(), None, None)],
            "no roster yet: no name (the handle is not written as the name)"
        );
        assert!(
            state.read_with(cx, |s, _| s.server_rosters_requested.contains(SERVER)),
            "the list asked for the server's members"
        );
        state.update(cx, |s, cx| s.on_server_detail(server(vec![scout()]), cx));
        assert_eq!(
            state.read_with(cx, |s, _| names(s)),
            vec![(
                "A".into(),
                Some("Scout".into()),
                Some("https://relay.test/a/scout.png".into())
            )],
            "named once the roster landed"
        );
    }

    /// A member with no display name set stays by handle (the display-name fallback).
    #[gpui::test]
    fn a_member_without_a_name_keeps_the_handle(cx: &mut TestAppContext) {
        let (_rt, state) = mount(cx);
        state.update(cx, |s, cx| {
            let mut nameless = scout();
            nameless.name = None;
            s.servers.push(server(vec![nameless]));
            s.on_matrix_timeline(
                ROOM.into(),
                vec![matrix_row(
                    "m1",
                    "@test_01scout:arbos.life",
                    "A",
                    "yo",
                    1_000,
                )],
                cx,
            );
        });
        let got = state.read_with(cx, |s, _| names(s));
        assert_eq!(got[0].0, "A");
        assert_eq!(
            got[0].1, None,
            "no name to show: display_name() falls back to the handle"
        );
        assert_eq!(
            got[0].2.as_deref(),
            Some("https://relay.test/a/scout.png"),
            "the picture still comes from the roster"
        );
    }
}

#[cfg(test)]
mod people_sheet_matcher_tests {
    //! Jacob, Mac v0.6.268, 2026-10-26 09:16 CR: "Give a person access" with
    //! "aff" typed listed only "Andreas F @aff" — the sheet showed the relay's
    //! handle-prefix search and never looked at display names. Every people
    //! sheet now lists the directory through the shared matcher.
    use super::*;

    fn person(handle: &str, name: &str) -> Person {
        Person {
            id: format!("gh_{handle}"),
            handle: handle.into(),
            name: (!name.is_empty()).then(|| name.to_string()),
            ..Default::default()
        }
    }

    fn directory() -> Vec<Person> {
        vec![
            person("doug", "Doug"),
            person("team-affine", "Team Affine"),
            person("aff", "Andreas F"),
            person("affine-foundation", "Affine"),
            person("robertalanm", "Carro"),
            person("affteam", "AffineTeam"),
        ]
    }

    fn handles(v: &[Person]) -> Vec<&str> {
        v.iter().map(|p| p.handle.as_str()).collect()
    }

    #[test]
    fn aff_lists_display_names_and_the_handle_ranked() {
        let rows = people_sheet_results(&directory(), &[], "aff");
        assert_eq!(
            handles(&rows),
            ["aff", "affine-foundation", "affteam", "team-affine"],
            "exact @aff, then the names that start with aff (directory order), then the word-start"
        );
        // The relay's handle-prefix answer alone is what Jacob saw.
        let relay_only = vec![person("aff", "Andreas F")];
        assert_eq!(
            handles(&people_sheet_results(&[], &relay_only, "aff")),
            ["aff"]
        );
    }

    #[test]
    fn relay_extras_join_the_directory_once() {
        let relay = vec![person("aff", "Andreas F"), person("affable", "Affable Bot")];
        let rows = people_sheet_results(&directory(), &relay, "@Aff");
        assert_eq!(
            handles(&rows),
            [
                "aff",
                "affine-foundation",
                "affteam",
                "affable",
                "team-affine"
            ]
        );
        assert_eq!(
            rows.iter().filter(|p| p.handle == "aff").count(),
            1,
            "one row per handle"
        );
    }

    #[test]
    fn empty_query_keeps_the_directory_order_and_diacritics_fold() {
        assert_eq!(
            handles(&people_sheet_results(&directory(), &[], "")),
            handles(&directory())
        );
        let dir = vec![person("zoe-m", "Zoë Martín"), person("jose", "José")];
        assert_eq!(
            handles(&people_sheet_results(&dir, &[], "martin")),
            ["zoe-m"]
        );
        assert_eq!(handles(&people_sheet_results(&dir, &[], "JOSÉ")), ["jose"]);
        assert_eq!(
            handles(&people_sheet_results(&dir, &[], "zoe mar")),
            ["zoe-m"]
        );
        assert!(
            people_sheet_results(&dir, &[], "oe").is_empty(),
            "no mid-word substrings"
        );
    }
}

#[cfg(test)]
mod fork_tests {
    //! #1257: a session fork stays inside its agent. It never becomes a project.
    use super::*;

    fn agent(id: &str, name: &str) -> Project {
        Project {
            id: id.into(),
            name: name.into(),
            ..Default::default()
        }
    }

    fn chat(id: &str, project: &str, title: &str, pinned: bool) -> Chat {
        Chat {
            id: id.into(),
            project_id: project.into(),
            title: title.into(),
            is_pinned: pinned,
            is_system: false,
            kind: ChatKind::Chat,
            participants: Vec::new(),
            unread: false,
            unread_count: 0,
            last_message_at: 0,
            archived_at: None,
        }
    }

    #[test]
    fn fork_is_a_sibling_chat_not_a_project() {
        let bob = agent("p-bob", "Bob");
        let unicorns = chat("c-unicorns", "p-bob", "unicorns", false);
        let fork = forked_chat(&unicorns, &bob, "c-local".into()).expect("forkable");
        assert_eq!(fork.project_id, bob.id, "same agent");
        assert_eq!(fork.title, "unicorns (fork)");
        assert_eq!(fork.kind, ChatKind::Chat);
        assert!(!fork.is_pinned);
        assert_eq!(fork_title(&fork.title), "unicorns (fork)");
        // The pinned "Main" chat forks under its display name, not the agent's.
        let main = chat("c-main", "p-bob", "Bob", true);
        let fork = forked_chat(&main, &bob, "c-local-2".into()).expect("forkable");
        assert_eq!(fork.title, "Main (fork)");
    }

    #[test]
    fn groups_system_tabs_and_shared_spaces_do_not_fork() {
        let bob = agent("p-bob", "Bob");
        let mut group = chat("c-g", "p-bob", "Group · Mira", false);
        group.kind = ChatKind::Group;
        assert!(forked_chat(&group, &bob, "x".into()).is_none());
        let mut system = chat("c-s", "p-bob", "Projects ⇄", false);
        system.is_system = true;
        assert!(forked_chat(&system, &bob, "x".into()).is_none());
        let mut shared = agent("p-shared", "Team");
        shared.shared = true;
        let c = chat("c-t", "p-shared", "Plan", false);
        assert!(forked_chat(&c, &shared, "x".into()).is_none());
        let other = chat("c-o", "p-other", "Elsewhere", false);
        assert!(forked_chat(&other, &bob, "x".into()).is_none());
    }
}

#[cfg(test)]
mod trim_tests {
    use super::{decode_thumbnail, opaque_rect, trim_rect};
    use image::{Rgba, RgbaImage};

    fn page(w: u32, h: u32, top: u32, left: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            if x >= left && x < w - left && y >= top && y < h - 40 {
                Rgba([30, 30, 30, 255])
            } else {
                Rgba([255, 255, 255, 255])
            }
        })
    }

    #[test]
    fn a_white_page_margin_is_trimmed_with_a_small_breathing_pad() {
        // 640 × 400, content from y 90 and x 40: the big top band goes, 2 % stays.
        let img = page(640, 400, 90, 40);
        let (x, y, w, h) = trim_rect(&img).expect("trims");
        assert_eq!((x, y), (40 - 12, 90 - 8));
        assert_eq!((w, h), (640 - 2 * 40 + 2 * 12, 400 - 90 - 40 + 2 * 8));
    }

    #[test]
    fn a_tight_picture_and_a_plain_swatch_stay_whole() {
        let tight = RgbaImage::from_fn(320, 200, |x, y| {
            if (2..318).contains(&x) && (2..198).contains(&y) {
                Rgba([30, 30, 30, 255])
            } else {
                Rgba([255, 255, 255, 255])
            }
        });
        assert!(trim_rect(&tight).is_none(), "nothing to trim");
        let swatch = RgbaImage::from_pixel(200, 200, Rgba([20, 120, 220, 255]));
        assert!(
            trim_rect(&swatch).is_none(),
            "uniform picture is not cropped away"
        );
    }

    #[test]
    fn a_thin_sliver_of_content_is_not_over_trimmed() {
        // Content 10 px tall in a 400 px image: the quarter floor keeps the picture.
        let img = RgbaImage::from_fn(640, 400, |_, y| {
            if (195..205).contains(&y) {
                Rgba([0, 0, 0, 255])
            } else {
                Rgba([255, 255, 255, 255])
            }
        });
        assert!(trim_rect(&img).is_none());
    }

    /// A macOS window capture: the window at (100, 100)–(600, 560) on
    /// transparent padding, a faint shadow below it. The staged thumbnail
    /// keeps the window only ([feedback composer-attachment-removal-2026-10-27]).
    #[test]
    fn a_window_captures_shadow_and_padding_are_cropped_from_the_thumbnail() {
        let img = RgbaImage::from_fn(700, 700, |x, y| {
            if (100..600).contains(&x) && (100..560).contains(&y) {
                Rgba([30, 30, 33, 255])
            } else if (80..620).contains(&x) && (120..620).contains(&y) {
                Rgba([0, 0, 0, 90])
            } else {
                Rgba([0, 0, 0, 0])
            }
        });
        assert_eq!(opaque_rect(&img), Some((100, 100, 500, 460)));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("png");
        let thumb = decode_thumbnail(&png, 256).expect("decodes");
        let size = thumb.size(0);
        assert_eq!(
            (size.width.0, size.height.0),
            (256, 236),
            "500 × 460 → 256 wide"
        );
        // A fully opaque picture is left alone (only shrunk).
        let opaque = RgbaImage::from_pixel(640, 360, Rgba([20, 120, 220, 255]));
        assert!(opaque_rect(&opaque).is_none());
    }
}

#[cfg(test)]
mod new_tab_scope_tests {
    //! Jacob (2026-10-23, Arbonites group): ⌘T in a group chat tried to create
    //! a project chat and the relay answered "no such project". New tab is an
    //! agent-space affordance only — the same gate decides the shortcut, the
    //! pill's "+" and the dropdown row, so none of them exist for people rooms
    //! or channels.
    use super::*;
    use crate::model::SpaceKind;

    fn space(kind: SpaceKind) -> Project {
        Project {
            id: "s-1".into(),
            name: "Space".into(),
            kind,
            ..Default::default()
        }
    }

    #[test]
    fn agent_space_keeps_new_tab() {
        assert!(new_tab_allowed(
            true,
            Some(&space(SpaceKind::Project)),
            false
        ));
    }

    #[test]
    fn people_rooms_and_channels_have_no_new_tab() {
        assert!(!new_tab_allowed(
            true,
            Some(&space(SpaceKind::Group)),
            false
        ));
        assert!(!new_tab_allowed(true, Some(&space(SpaceKind::Dm)), false));
        // A channel in the centre: no space selected, channel open.
        assert!(!new_tab_allowed(true, None, true));
        // Nothing open at all.
        assert!(!new_tab_allowed(true, None, false));
    }

    #[test]
    fn multi_chat_off_hides_it_everywhere() {
        assert!(!new_tab_allowed(
            false,
            Some(&space(SpaceKind::Project)),
            false
        ));
    }
}

#[cfg(test)]
mod call_strip_tests {
    //! The call strip resizes inside the chat column; the sidebar width is a different
    //! field that no call path touches (Jacob, 2026-10-23: the sidebar stays normal).

    use super::*;

    #[test]
    fn the_strip_height_is_separate_from_the_sidebar_width() {
        let strip = crate::ui::callview::strip_layout::DEFAULT_HEIGHT;
        let sidebar = crate::ui::theme::SidebarWidth::clamp(300.0);
        let next = crate::ui::callview::strip_layout::height_after_drag(strip, 60.0, 780.0);
        // The same clamp the drag uses leaves the sidebar's own clamp untouched.
        assert_eq!(next, strip + 60.0);
        assert_eq!(crate::ui::theme::SidebarWidth::clamp(sidebar), sidebar);
        // And an ActiveCall carries nothing about the sidebar: ending it only drops `my_call`.
        let active = ActiveCall {
            call: Call::default(),
            status: CallStatus::Connected,
            muted: false,
            camera: false,
            participants: Vec::new(),
            local_video: None,
            joined_at: 0,
            expanded: true,
            notice: None,
            outgoing: false,
            reachable: None,
            agent_seeing: false,
        };
        assert!(active.expanded);
    }
}

#[cfg(test)]
mod chat_image_scale_tests {
    use super::{downscale_for_chat, CHAT_IMAGE_MAX_SIDE};
    use image::{Rgba, RgbaImage};

    #[test]
    fn a_small_picture_is_left_alone() {
        let img = RgbaImage::from_pixel(320, 200, Rgba([10, 20, 30, 255]));
        let out = downscale_for_chat(img);
        assert_eq!(out.dimensions(), (320, 200));
    }

    #[test]
    fn a_huge_screenshot_fits_the_chat_budget() {
        // Typical Retina capture: far past the bubble's paint size.
        let img = RgbaImage::from_pixel(3024, 1964, Rgba([40, 40, 40, 255]));
        let out = downscale_for_chat(img);
        let (w, h) = out.dimensions();
        assert_eq!(w.max(h), CHAT_IMAGE_MAX_SIDE);
        assert!(w.min(h) < CHAT_IMAGE_MAX_SIDE);
        // Aspect ≈ 3024/1964.
        let ratio = w as f32 / h as f32;
        assert!((ratio - 3024.0 / 1964.0).abs() < 0.02);
    }
}

#[cfg(test)]
mod animated_image_tests {
    //! GIF / animated WebP decode to every frame; still WebP and PNG stay one
    //! (feedback app-01M5TGH18WV3QCK5KZT6XKG7FG / app-01M61EZD0GJ2XGV1EW4SCVXENK).
    use super::{
        animation_frame_side, decode_image, decode_image_pair, is_https_page_url,
        ANIM_MAX_DECODED_BYTES, ANIM_MAX_FRAMES, ANIM_SIDE_FLOOR, ANIM_SIDE_START,
    };
    use image::{Delay, Frame, Rgba, RgbaImage};
    use std::io::Cursor;

    fn three_color_frames() -> Vec<Frame> {
        [
            ([255, 0, 0, 255], 100u32),
            ([0, 255, 0, 255], 100),
            ([0, 0, 255, 255], 100),
        ]
        .into_iter()
        .map(|(c, ms)| {
            Frame::from_parts(
                RgbaImage::from_pixel(8, 8, Rgba(c)),
                0,
                0,
                Delay::from_numer_denom_ms(ms, 1),
            )
        })
        .collect()
    }

    fn encode_gif(frames: Vec<Frame>) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = image::codecs::gif::GifEncoder::new(Cursor::new(&mut out));
            enc.encode_frames(frames).expect("gif encode");
        }
        out
    }

    fn encode_webp_anim(frames: Vec<Frame>) -> Vec<u8> {
        // image::codecs::webp::WebPEncoder is still-only; build via the
        // `image-webp` path used by the decoder tests — encode a GIF and
        // re-wrap is not WebP. Use the fixture bytes for WebP instead.
        let _ = frames;
        include_bytes!("../../arbos-core/fixtures/anim_three.webp").to_vec()
    }

    #[test]
    fn animation_side_fits_common_reaction_gifs() {
        assert_eq!(animation_frame_side(60, 480, 270), Some(480));
        let side = animation_frame_side(150, 480, 480).expect("150 square frames animate");
        assert!((ANIM_SIDE_FLOOR..=ANIM_SIDE_START).contains(&side));
        let side = animation_frame_side(300, 1000, 1000).expect("300 large frames animate");
        assert!(side >= ANIM_SIDE_FLOOR);
        let (sw, sh) = {
            let scale = side as f64 / 1000.0;
            (
                (1000.0 * scale).round().max(1.0) as usize,
                (1000.0 * scale).round().max(1.0) as usize,
            )
        };
        assert!(
            300usize
                .saturating_mul(sw)
                .saturating_mul(sh)
                .saturating_mul(4)
                <= ANIM_MAX_DECODED_BYTES
        );
        assert_eq!(animation_frame_side(301, 480, 270), None);
    }

    #[test]
    fn https_page_url_accepts_only_https() {
        assert!(is_https_page_url("https://example.com/g/1"));
        assert!(is_https_page_url("HTTPS://Example.COM/g"));
        assert!(!is_https_page_url("http://example.com/g/1"));
        assert!(!is_https_page_url("file:///tmp/x.gif"));
        assert!(!is_https_page_url("javascript:alert(1)"));
        assert!(!is_https_page_url("arbos://gif/1"));
        assert!(!is_https_page_url("not a url"));
    }

    #[test]
    fn a_three_frame_gif_keeps_every_frame() {
        let bytes = encode_gif(three_color_frames());
        let image = decode_image(&bytes).expect("gif decodes");
        assert_eq!(image.frame_count(), 3);
        let (full, trimmed) = decode_image_pair(&bytes).expect("pair");
        assert_eq!(full.frame_count(), 3);
        assert!(trimmed.is_none(), "animated pictures are not trimmed");
    }

    #[test]
    fn a_120_frame_reaction_gif_animates_within_the_cap() {
        let delay = Delay::from_numer_denom_ms(30, 1);
        let frames: Vec<Frame> = (0..120)
            .map(|i| {
                let c = (i % 255) as u8;
                Frame::from_parts(
                    RgbaImage::from_pixel(480, 270, Rgba([c, 0, 255 - c, 255])),
                    0,
                    0,
                    delay,
                )
            })
            .collect();
        let bytes = encode_gif(frames);
        let image = decode_image(&bytes).expect("decodes");
        assert_eq!(image.frame_count(), 120);
    }

    #[test]
    fn an_animated_webp_keeps_every_frame() {
        let bytes = encode_webp_anim(three_color_frames());
        let image = decode_image(&bytes).expect("webp decodes");
        assert_eq!(image.frame_count(), 3);
    }

    #[test]
    fn a_still_webp_and_a_png_are_one_frame() {
        let still_webp = include_bytes!("../../arbos-core/fixtures/still.webp");
        assert_eq!(
            decode_image(still_webp).expect("still webp").frame_count(),
            1
        );
        let png = include_bytes!("../../arbos-core/fixtures/anim_still.png");
        assert_eq!(decode_image(png).expect("png").frame_count(), 1);
    }

    #[test]
    fn past_the_frame_cap_falls_back_to_the_first_frame() {
        // Many tiny frames: the decoder stops at ANIM_MAX_FRAMES and returns
        // only the first as a still.
        let delay = Delay::from_numer_denom_ms(20, 1);
        let frames: Vec<Frame> = (0..(ANIM_MAX_FRAMES + 5))
            .map(|i| {
                let c = (i % 255) as u8;
                Frame::from_parts(
                    RgbaImage::from_pixel(2, 2, Rgba([c, 0, 0, 255])),
                    0,
                    0,
                    delay,
                )
            })
            .collect();
        let bytes = encode_gif(frames);
        let image = decode_image(&bytes).expect("decodes");
        assert_eq!(
            image.frame_count(),
            1,
            "over {ANIM_MAX_FRAMES} frames → first only"
        );
        // Memory cap is the sibling of the frame cap (same fallback path).
        let _ = ANIM_MAX_DECODED_BYTES;
    }
}

#[cfg(test)]
mod call_video_frame_tests {
    //! feedback app-01M5R1Z6356QE63XK9EP4HDGEG: each decoded video frame was a
    //! new RenderImage left in the sprite atlas. swap_frame / remove_call_participant
    //! hand the previous frame back so App::drop_image can free it.
    use super::{remove_call_participant, swap_frame, ActiveCall, CallParticipant, CallStatus};
    use crate::model::{Call, Sender};
    use gpui::RenderImage;
    use std::sync::Arc;

    fn frame() -> Arc<RenderImage> {
        Arc::new(RenderImage::new(Vec::new()))
    }

    fn active_with(participant: CallParticipant) -> ActiveCall {
        ActiveCall {
            call: Call::default(),
            status: CallStatus::Connected,
            muted: false,
            camera: true,
            participants: vec![participant],
            local_video: None,
            joined_at: 0,
            expanded: true,
            notice: None,
            outgoing: false,
            reachable: None,
            agent_seeing: false,
        }
    }

    #[test]
    fn swap_frame_returns_none_the_first_time() {
        let mut slot = None;
        let first = frame();
        assert!(swap_frame(&mut slot, Some(first.clone())).is_none());
        assert!(Arc::ptr_eq(slot.as_ref().unwrap(), &first));
    }

    #[test]
    fn swap_frame_returns_the_previous_frame_and_keeps_the_new_one() {
        let mut slot = None;
        let first = frame();
        let second = frame();
        assert!(swap_frame(&mut slot, Some(first.clone())).is_none());
        let prev = swap_frame(&mut slot, Some(second.clone()));
        assert!(Arc::ptr_eq(prev.as_ref().unwrap(), &first));
        assert!(Arc::ptr_eq(slot.as_ref().unwrap(), &second));
        assert!(swap_frame(&mut slot, None).is_some_and(|p| Arc::ptr_eq(&p, &second)));
        assert!(slot.is_none());
    }

    #[test]
    fn remove_call_participant_hands_back_that_participants_frame() {
        let video = frame();
        let mut active = active_with(CallParticipant {
            identity: "u_ken".into(),
            sender: Sender::default(),
            speaking: false,
            muted: false,
            video: Some(video.clone()),
        });
        active.participants.push(CallParticipant {
            identity: "u_other".into(),
            sender: Sender::default(),
            speaking: false,
            muted: false,
            video: Some(frame()),
        });
        let taken = remove_call_participant(&mut active, "u_ken");
        assert!(taken.is_some_and(|p| Arc::ptr_eq(&p, &video)));
        assert_eq!(active.participants.len(), 1);
        assert_eq!(active.participants[0].identity, "u_other");
        assert!(remove_call_participant(&mut active, "u_missing").is_none());
    }
}

#[cfg(test)]
mod forward_filter_tests {
    //! feedback forward-sheet-focus-2026-10-24: typing in the "Forward to"
    //! field filters the list (once keys reach the field at all).
    use super::{filter_forward_targets, ForwardTarget};

    fn target(name: &str, subtitle: &str, at: u64) -> ForwardTarget {
        ForwardTarget {
            space_id: format!("s-{name}"),
            chat_id: format!("c-{name}"),
            name: name.into(),
            subtitle: subtitle.into(),
            last_activity: at,
        }
    }

    fn all() -> Vec<ForwardTarget> {
        vec![
            target("Arbos", "Agent", 10),
            target("sam", "@sam", 30),
            target("Launch week", "Group · 5 members", 20),
            target("Samir Haddad", "@samir", 5),
        ]
    }

    fn names(v: &[ForwardTarget]) -> Vec<&str> {
        v.iter().map(|t| t.name.as_str()).collect()
    }

    #[test]
    fn an_empty_query_keeps_every_chat_most_recent_first() {
        assert_eq!(
            names(&filter_forward_targets(all(), "  ")),
            ["sam", "Launch week", "Arbos", "Samir Haddad"]
        );
    }

    #[test]
    fn typing_narrows_by_name_or_subtitle_case_insensitively() {
        assert_eq!(
            names(&filter_forward_targets(all(), "sam")),
            ["sam", "Samir Haddad"]
        );
        assert_eq!(
            names(&filter_forward_targets(all(), "SAMI")),
            ["Samir Haddad"]
        );
        assert_eq!(
            names(&filter_forward_targets(all(), "group")),
            ["Launch week"]
        );
        assert_eq!(names(&filter_forward_targets(all(), "agent")), ["Arbos"]);
        assert!(filter_forward_targets(all(), "zzz").is_empty());
    }
}

impl search_index::SenderName for AppState {
    fn sender_name(&self, _chat_id: &str, message: &Message) -> String {
        self.viewer_sender_name(message)
    }
}

// ----- ⌘F Search-in-chat ([feedback search-in-chat-mac-2026-10-26]) ----------------

/// The open Search-in-chat bar: its chat, what was typed, the pinned sender,
/// the hits over the loaded rows and the cursor. The pure rules live in
/// `crate::chat_search`; the view in `ui::chat_search`.
#[derive(Clone, Debug, Default)]
pub struct ChatSearch {
    pub chat_id: Id,
    /// The field's text as typed.
    pub typed: String,
    pub query: chat_search::Query,
    /// Message ids the query hits, transcript order (oldest first).
    pub hits: Vec<Id>,
    /// Index into `hits` of the revealed message.
    pub current: Option<usize>,
    /// ‹ on the oldest loaded hit asked the relay for older rows; cleared
    /// when they land (or when the room has none).
    pub searching_older: bool,
    /// The room answered "no older rows": ‹ wraps from now on.
    pub exhausted: bool,
    /// How many rows the chat had at the last recompute (a page that brings
    /// none ends the search upward).
    pub rows_seen: usize,
}

/// What the transcript draws for one message while a search is up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchMarks {
    /// The folded keyword to mark in the text (empty with a sender-only query).
    pub needle: String,
    /// This message is the revealed hit (Find's yellow on its words).
    pub current: bool,
}

impl AppState {
    /// The bar is up for the chat the centre shows.
    pub fn chat_search_open(&self) -> bool {
        self.chat_search
            .as_ref()
            .is_some_and(|s| Some(&s.chat_id) == self.active_chat_id().as_ref())
    }

    /// ⌘F: open the bar for the active chat (a bar left on another chat is
    /// replaced). Returns false with no chat open.
    pub fn open_chat_search(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(chat_id) = self.active_chat_id() else {
            return false;
        };
        if !self.chat_search_open() {
            self.chat_search = Some(ChatSearch {
                chat_id,
                ..Default::default()
            });
        }
        cx.notify();
        true
    }

    /// Esc / ✕: the bar goes, the marks and the revealed row with it.
    pub fn close_chat_search(&mut self, cx: &mut Context<Self>) {
        if let Some(search) = self.chat_search.take() {
            if let Some(cur) = search.current.and_then(|i| search.hits.get(i)) {
                if self.highlight_message.as_ref() == Some(cur) {
                    self.highlight_message = None;
                }
            }
            cx.notify();
        }
    }

    /// The field changed: new keyword, hits recomputed, cursor on the newest hit.
    pub fn chat_search_set_text(&mut self, typed: String, cx: &mut Context<Self>) {
        let Some(search) = self.chat_search.as_mut() else {
            return;
        };
        if search.typed == typed {
            return;
        }
        search.typed = typed;
        search.exhausted = false;
        self.chat_search_recompute(true, cx);
    }

    /// ⇥ or a click on the offered chip: pin that sender, clear the text.
    pub fn chat_search_pin_sender(&mut self, sender: chat_search::Sender, cx: &mut Context<Self>) {
        let Some(search) = self.chat_search.as_mut() else {
            return;
        };
        search.query.sender = Some(sender);
        search.typed.clear();
        search.exhausted = false;
        self.chat_search_recompute(true, cx);
    }

    /// The chip's ✕ (or ⌫ on an empty field): the sender filter goes.
    pub fn chat_search_unpin_sender(&mut self, cx: &mut Context<Self>) {
        let Some(search) = self.chat_search.as_mut() else {
            return;
        };
        if search.query.sender.take().is_none() {
            return;
        }
        self.chat_search_recompute(true, cx);
    }

    /// The sender the typed text names, offered as a chip while none is pinned.
    pub fn chat_search_offer(&self) -> Option<chat_search::Sender> {
        let search = self.chat_search.as_ref()?;
        if search.query.sender.is_some() {
            return None;
        }
        let senders = chat_search::senders(&self.chat_search_rows(&search.chat_id));
        chat_search::offer_sender(&search.typed, &senders).cloned()
    }

    /// Enter / › (+1), ⇧Enter / ‹ (−1), ⌘G / ⇧⌘G: move the cursor, reveal the
    /// hit; ‹ on the oldest loaded hit of a channel pages older rows first.
    pub fn chat_search_step(&mut self, delta: i32, cx: &mut Context<Self>) {
        let Some(search) = self.chat_search.as_ref() else {
            return;
        };
        let (chat_id, current, len, exhausted) = (
            search.chat_id.clone(),
            search.current,
            search.hits.len(),
            search.exhausted,
        );
        let may_have_older = !exhausted && self.chat_search_pages(&chat_id);
        match chat_search::step(current, len, delta, may_have_older) {
            chat_search::Step::To(i) => {
                if let Some(search) = self.chat_search.as_mut() {
                    search.current = Some(i);
                }
                self.chat_search_reveal(cx);
            }
            chat_search::Step::PageOlder => {
                if let Some(search) = self.chat_search.as_mut() {
                    search.searching_older = true;
                }
                self.command(ClientCommand::ChannelLoadOlder { room_id: chat_id });
                cx.notify();
            }
            chat_search::Step::Nothing => {}
        }
    }

    /// Rows changed in the search's chat (a new message, a channel page):
    /// hits recomputed, the cursor kept on its message; a page that was asked
    /// for by ‹ moves on to the next older hit.
    pub fn chat_search_refresh(&mut self, chat_id: &str, cx: &mut Context<Self>) {
        let Some(search) = self.chat_search.as_ref() else {
            return;
        };
        if search.chat_id != chat_id {
            return;
        }
        let was_paging = search.searching_older;
        let oldest_before = search.hits.first().cloned();
        let rows_before = search.rows_seen;
        self.chat_search_recompute(false, cx);
        if !was_paging {
            return;
        }
        let Some(search) = self.chat_search.as_mut() else {
            return;
        };
        let rows_now = search.rows_seen;
        search.searching_older = false;
        let older_hit = match &oldest_before {
            Some(id) => search
                .hits
                .iter()
                .position(|h| h == id)
                .filter(|p| *p > 0)
                .map(|p| p - 1),
            // No hit before the page: the newest of the ones it brought.
            None => chat_search::initial_cursor(search.hits.len()),
        };
        if let Some(i) = older_hit {
            search.current = Some(i);
            self.chat_search_reveal(cx);
        } else if rows_now == rows_before {
            // The page brought nothing: the room's history is exhausted upward.
            search.exhausted = true;
            cx.notify();
        } else {
            // Rows came but none hit: keep paging.
            self.chat_search_step(-1, cx);
        }
    }

    /// The room has no older rows: ‹ wraps from now on.
    pub fn chat_search_history_end(&mut self, room_id: &str, cx: &mut Context<Self>) {
        if let Some(search) = self.chat_search.as_mut() {
            if search.chat_id == room_id && search.searching_older {
                search.searching_older = false;
                search.exhausted = true;
                cx.notify();
            }
        }
    }

    /// What the transcript marks on `message_id` while the bar is up.
    pub fn chat_search_marks(&self, message_id: &str) -> Option<SearchMarks> {
        let search = self.chat_search.as_ref()?;
        if !self.chat_search_open() {
            return None;
        }
        let at = search.hits.iter().position(|h| h == message_id)?;
        Some(SearchMarks {
            needle: search.query.keyword.clone(),
            current: search.current == Some(at),
        })
    }

    /// "2 of 4" / "No matches" / "" for the bar.
    pub fn chat_search_count(&self) -> String {
        self.chat_search
            .as_ref()
            .map(|s| chat_search::count_label(&s.query, s.current, s.hits.len()))
            .unwrap_or_default()
    }

    /// Can the chat page older rows at all: only server channels fetch
    /// history from the relay; a chat doc is whole on this Mac.
    fn chat_search_pages(&self, chat_id: &str) -> bool {
        self.channel_rows.contains_key(chat_id)
    }

    fn chat_search_recompute(&mut self, reset_cursor: bool, cx: &mut Context<Self>) {
        let Some(search) = self.chat_search.as_ref() else {
            return;
        };
        let rows = self.chat_search_rows(&search.chat_id);
        let query = chat_search::Query::new(&search.typed, search.query.sender.clone());
        let hits = chat_search::hits(&rows, &query);
        let was_on = search.current.and_then(|i| search.hits.get(i)).cloned();
        let current = if reset_cursor {
            chat_search::initial_cursor(hits.len())
        } else {
            chat_search::keep_cursor(&hits, was_on.as_ref())
        };
        let moved = current.and_then(|i| hits.get(i)) != was_on.as_ref();
        let rows_seen = rows.len();
        if let Some(search) = self.chat_search.as_mut() {
            search.query = query;
            search.hits = hits;
            search.current = current;
            search.rows_seen = rows_seen;
        }
        if moved {
            self.chat_search_reveal(cx);
        }
        cx.notify();
    }

    /// Scroll to and highlight the current hit through the palette's reveal
    /// path (`reveal_search_message` → `highlight_message` + `reveal_message`
    /// → `Transcript::reveal_hit`): one reveal mechanism, not a second.
    fn chat_search_reveal(&mut self, cx: &mut Context<Self>) {
        let Some(search) = self.chat_search.as_ref() else {
            return;
        };
        let Some(id) = search.current.and_then(|i| search.hits.get(i)).cloned() else {
            self.highlight_message = None;
            cx.notify();
            return;
        };
        self.highlight_message = Some(id.clone());
        self.reveal_message = Some((search.chat_id.clone(), id));
        self.reveal_nonce += 1;
        cx.notify();
    }

    /// The loaded messages of `chat_id` as the search sees them (system rows
    /// and wordless job rows left out, like the palette's index).
    fn chat_search_rows(&self, chat_id: &str) -> Vec<chat_search::Row> {
        self.messages_of(chat_id)
            .iter()
            .filter(|m| m.role != Role::System && !m.hidden)
            .filter_map(|m| {
                let text = crate::ui::sidebar::plain(&m.plain_text());
                let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                if text.is_empty() {
                    return None;
                }
                Some(chat_search::Row {
                    id: m.id.clone(),
                    text,
                    sender: self.sender_identity(chat_id, m),
                })
            })
            .collect()
    }

    /// Who sent `m`, for the sender chip: display name, handle, picture. An
    /// agent's reply in its own chat (no handle, `sender` without an agent id)
    /// is the chat's agent.
    pub fn sender_identity(&self, chat_id: &str, m: &Message) -> chat_search::Sender {
        let name = self.viewer_sender_name(m);
        if self.is_mine(m) {
            return chat_search::Sender {
                key: format!("user:{}", self.me_id()),
                name: self.me_name(),
                handle: self.me_handle(),
                avatar_url: None,
            };
        }
        if let Some(h) = m.sender_handle.as_deref().filter(|h| !h.is_empty()) {
            let person = self.person_by_handle(h);
            return chat_search::Sender {
                key: format!("@{h}"),
                name,
                handle: h.to_owned(),
                avatar_url: person
                    .map(|p| p.avatar_url)
                    .filter(|u| !u.is_empty())
                    .or_else(|| m.sender_avatar.clone()),
            };
        }
        let agent = m
            .sender
            .strip_prefix("agent:")
            .or_else(|| m.sender.strip_prefix("project:"))
            .and_then(|pid| self.project(pid))
            .or_else(|| {
                (m.role == Role::Assistant)
                    .then(|| self.project(&self.space_of_chat(chat_id)))
                    .flatten()
                    .filter(|p| !p.is_people())
            });
        match agent {
            Some(p) => chat_search::Sender {
                key: format!("agent:{}", p.id),
                name: p.name.clone(),
                handle: Project::mention_slug(&p.name),
                avatar_url: m.sender_avatar.clone(),
            },
            None => chat_search::Sender {
                key: m.sender.clone(),
                name,
                handle: String::new(),
                avatar_url: m.sender_avatar.clone(),
            },
        }
    }
}

/// The event kind for the skip log, without its payload: the Debug output is cut at the
/// variant name (a snapshot's rows are never formatted).
fn sync_event_label(event: &SyncEvent) -> String {
    use std::fmt::Write as _;
    struct Head(String);
    impl std::fmt::Write for Head {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            for c in s.chars() {
                if matches!(c, ' ' | '(' | '{') || self.0.len() >= 48 {
                    return Err(std::fmt::Error);
                }
                self.0.push(c);
            }
            Ok(())
        }
    }
    let mut head = Head(String::new());
    let _ = write!(head, "{event:?}");
    if head.0.is_empty() {
        "event".to_owned()
    } else {
        head.0
    }
}
