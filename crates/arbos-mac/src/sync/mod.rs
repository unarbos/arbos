//! The data layer. A [`SyncSource`] feeds the UI with [`SyncEvent`]s and takes
//! [`ClientCommand`]s back. Two sources exist: [`fixture::FixtureSource`]
//! (local Loro docs, scripted agent) and [`relay::RelaySource`] (HTTP + one
//! WebSocket per room on `arbos-relay`, `crdt-sync.md` §7–§9).
//!
//! Everything in this module is GPUI-free. Wire frames and the doc schema
//! constants come from `arbos-core`; the client-side mirror (`doc.rs`) and
//! the sources are candidates to move there later (`arbos-design.md` §7.2).

pub mod agent_avatar;
pub mod avatar_master;
pub mod chat_send_queue;
pub mod credentials;
pub mod diagnostics;
pub mod doc;
pub mod dto;
pub mod fixture;
pub mod relay;
pub mod store;
pub mod user_error;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use arbos_matrix::{MessageRow, Snapshot};
use futures::future::BoxFuture;
use tokio::sync::mpsc;

use crate::sync::user_error::UserError;

use crate::model::{
    AccountView, Call, CallEnded, CallKind, CallMissed, Person, Reachable, Sender, ServerInfo,
    Voice,
};
use crate::model::{
    Attachment, BoxReport, BoxState, ChannelsView, ChartBody, Chat, ConnectorsView, DeviceView,
    Effort, Id, Link, LinkState, LocalView, Machine, Message, NewSecret, Project, ProjectDocView,
    ProjectSecrets, Routine, RoutineRun, RoutineTrigger, SecretsView, Skill,
};

/// Why the relay refused a people-room push ([feedback blocked-state-relay-2026-10-26]):
/// the frame's `code`, its `reason`, and — on a block — which way it runs and who the other
/// party is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PushRefusal {
    /// The wire code (`not_available`); empty when cleared.
    pub code: String,
    /// `blocked_by` (they blocked me) / `blocks_member` (I blocked them) / `silenced` /
    /// `chat_unavailable`; `None` from a relay that predates the field.
    pub reason: Option<String>,
    pub by_handle: Option<String>,
    pub by_name: Option<String>,
}

/// "Muted until 14:30" when the end is later today, "Muted until Oct 30, 14:30" on any
/// other day, "Muted" with no end. HH:MM is 24-hour local time in `tz`.
pub fn muted_until_label<Tz: chrono::TimeZone>(until_ms: Option<i64>, now_ms: i64, tz: Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let Some(until_ms) = until_ms else {
        return "Muted".into();
    };
    let Some(until) = tz.timestamp_millis_opt(until_ms).single() else {
        return "Muted".into();
    };
    let Some(now) = tz.timestamp_millis_opt(now_ms).single() else {
        return "Muted".into();
    };
    if until.date_naive() == now.date_naive() {
        format!("Muted until {}", until.format("%H:%M"))
    } else {
        format!("Muted until {}", until.format("%b %-d, %H:%M"))
    }
}

/// Clear a timed admin mute once `now_ms` is at or past the end. Also drops a
/// `push_refused` entry whose reason is `silenced`. Returns true when something changed.
pub fn clear_silence_if_expired(
    project: &mut Project,
    push_refused: &mut HashMap<Id, PushRefusal>,
    now_ms: i64,
) -> bool {
    if !project.silenced {
        return false;
    }
    let Some(until) = project.silenced_until else {
        return false;
    };
    if until > now_ms {
        return false;
    }
    let chat_id = project.id.clone();
    project.silenced = false;
    project.silenced_until = None;
    if push_refused
        .get(&chat_id)
        .is_some_and(|w| w.reason.as_deref() == Some("silenced"))
    {
        push_refused.remove(&chat_id);
    }
    true
}

impl PushRefusal {
    pub fn code(code: &str) -> Self {
        Self {
            code: code.to_owned(),
            ..Default::default()
        }
    }

    /// The other party's display name, else `@handle`, else nothing.
    pub fn who(&self) -> Option<String> {
        self.by_name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                self.by_handle
                    .as_deref()
                    .filter(|h| !h.is_empty())
                    .map(|h| format!("@{h}"))
            })
    }

    /// The person to unblock, when the sentence says "Unblock to post".
    pub fn unblock_handle(&self) -> Option<&str> {
        (self.reason.as_deref() == Some("blocks_member"))
            .then_some(self.by_handle.as_deref())
            .flatten()
            .filter(|h| !h.is_empty())
    }

    /// The plain sentence the chat-level note shows (Jacob 2026-10-26): who stands in the
    /// way and what to do; the generic line for anything else. `silenced_until_ms` is the
    /// active chat's end time for the mute line ([feedback app-01M61GZ5B66VCSKF40458KH3DP]).
    pub fn sentence(&self, silenced_until_ms: Option<i64>) -> String {
        self.sentence_at(
            silenced_until_ms,
            chrono::Local::now().timestamp_millis(),
            chrono::Local,
        )
    }

    /// Same as [`Self::sentence`], with a fixed clock and time zone (tests).
    pub fn sentence_at<Tz: chrono::TimeZone>(
        &self,
        silenced_until_ms: Option<i64>,
        now_ms: i64,
        tz: Tz,
    ) -> String
    where
        Tz::Offset: std::fmt::Display,
    {
        match (self.reason.as_deref(), self.who()) {
            (Some("blocked_by"), Some(who)) => format!("{who} has blocked you in this chat."),
            (Some("blocked_by"), None) => "Someone in this chat has blocked you.".to_string(),
            (Some("blocks_member"), Some(who)) => format!(
                "Couldn't post: you've blocked {who}, who is in this chat. Unblock to post."
            ),
            (Some("blocks_member"), None) => {
                "Couldn't post: you've blocked someone in this chat. Unblock to post.".to_string()
            }
            (Some("silenced"), _) => {
                format!(
                    "{}. Your messages aren't being delivered.",
                    muted_until_label(silenced_until_ms, now_ms, tz)
                )
            }
            _ => "Messages in this chat aren't being delivered right now. We'll keep trying."
                .to_string(),
        }
    }
}

/// Something the source learned; the UI applies it to its mirror.
#[derive(Clone, Debug)]
pub enum SyncEvent {
    /// No device token for this relay yet: show the pairing screen.
    NeedsPairing {
        relay_url: String,
    },
    /// An outside agent's key (#2141), shown once on the reveal sheet: after a connect or a
    /// rotate.
    OutsideAgentKey {
        project_id: Id,
        name: String,
        transport: String,
        key: String,
        link: String,
        relay_url: String,
        /// The agent's 1:1 chat (connect only).
        chat_id: Option<Id>,
        rotated: bool,
    },
    /// The connection of an outside agent, polled while the reveal sheet is up and on the
    /// Details pane.
    OutsideAgentStatus {
        project_id: Id,
        info: crate::model::OutsideInfo,
    },
    /// The Wake URL form was saved (`PATCH /outside-agents/{id}`); `error` is the relay's
    /// short reason when it refused.
    OutsideAgentWakeSaved {
        project_id: Id,
        error: Option<String>,
    },
    /// "Send test wake" came back: `text` is "Delivered · HTTP 200" or the short error.
    OutsideAgentWakeTested {
        project_id: Id,
        ok: bool,
        text: String,
    },
    /// Who is signed in (`/auth/token` or `/me`); `None` after sign-out.
    Account(Option<AccountView>),
    /// Remote feature flags from `GET /me` → `features` (one object, one place on the relay).
    Features {
        /// `features.voice_mode`: spoken replies, voice picker, reply playback. Absent on an
        /// older relay = off (Jacob, 2026-10-22: hidden everywhere until it works end to end).
        voice_mode: bool,
    },
    /// `GET /users/search` results for the new-chat sheet (D72).
    PeopleSearch {
        query: String,
        results: Vec<Person>,
    },
    /// Someone in a people chat started/stopped typing (room presence).
    /// Someone's device sent a presence frame in a room we share (API §7c
    /// frames carry `handle`): they are online right now. Optional `seen` is
    /// the message id they last looked at (read receipts).
    Presence {
        handle: String,
        chat_id: Option<Id>,
        seen: Option<Id>,
    },
    Typing {
        chat_id: Id,
        handle: String,
        active: bool,
    },
    /// Avatar bytes for an `avatar_url` (PNG/JPEG), fetched once.
    Avatar {
        url: String,
        bytes: Arc<Vec<u8>>,
    },
    /// The picture at `url` could not be fetched (retries exhausted, or 404): the app forgets
    /// the request so the next surface that wants it asks again.
    AvatarMissing {
        url: String,
    },
    /// The avatar picker's generated picture is ready to preview (`url` fetches it; `image_key`
    /// is what "Set avatar" sends), or the generation failed (`error`).
    ImageGenerated {
        target: PictureTarget,
        image_key: Option<String>,
        url: Option<String>,
        /// A short human sentence for the sheet ("Couldn't generate the image. The server took
        /// too long."); the raw provider / transport words ride in `detail` for a Details
        /// disclosure (Jacob, 2026-10-26: a raw reqwest error had spilled over the footer).
        error: Option<UserError>,
    },
    /// Error from a people-chat action (search, DM, members), shown inline.
    PeopleError(UserError),
    /// The people I block (`GET /blocks`), sent on every registry refresh and after each
    /// block / unblock. The person pane derives its Block user / Unblock row from this list.
    Blocked(Vec<Person>),
    /// `CreatePeopleGroup` succeeded: the new group's id, sent after the
    /// registry refresh that lists it. The sheet closes and the group opens.
    PeopleGroupCreated {
        chat_id: Id,
    },
    /// Calls v1: `POST …/calls` or `/join` answered with a call (and my token).
    /// `created` = 201: this device started it and writes the system line;
    /// `reachable` (201 only) says per callee whether anyone can pick up.
    CallReady {
        call: Call,
        created: bool,
        reachable: Vec<Reachable>,
    },
    /// Someone else started a call in one of my chats (`call_started` push).
    CallStarted {
        call: Call,
        sender: Sender,
    },
    /// A message in a server channel (`channel_message` push from the relay's Matrix push
    /// gateway): the banner source for servers, which never pass through the chat docs.
    ChannelMessage {
        server_id: String,
        server_name: String,
        channel_id: String,
        channel_name: String,
        room_id: String,
        sender: Sender,
        text: Option<String>,
    },
    /// A call ended (push, or my `leave` answered `ended: true`).
    CallEnded(CallEnded),
    /// A call in one of my chats ended and I never joined (`call_missed` push).
    CallMissed(CallMissed),
    /// `GET /calls/{id}` answer (live or ended).
    CallInfo(Call),
    /// A presence frame in a chat named a call this device is not in.
    CallSeen {
        chat_id: Id,
        call_id: String,
    },
    /// A call request failed; shown as an error toast and the call UI resets.
    CallError {
        chat_id: Id,
        error: UserError,
    },
    /// Cold launch from the on-device store (#929): reopen what was on
    /// screen when the app last ran. Sent once, after the stored `Projects`.
    RestoreSelection {
        project_id: Id,
        chat_id: Id,
    },
    /// From the on-device store, once at launch: the chat last used per agent
    /// and the chats archived on this Mac.
    ChatPrefs {
        last_chats: HashMap<Id, Id>,
        archived: Vec<Id>,
    },
    /// §9c: a chat's archive state changed for me (another device, `chat_archived`
    /// push) — `None` = unarchived.
    ChatArchived {
        chat_id: Id,
        archived_at: Option<u64>,
    },
    /// My sidebar pins, top first (`GET /me/pins`, the local cache at launch, a
    /// `pins_changed` push from another device, or the relay's answer to `SetPins`):
    /// row keys `agent:<project>` / `chat:<people chat>` / `server:<server>`; `cap` is
    /// the relay's limit ([feedback sidebar-pinning-mac-2026-10-28]).
    Pins {
        pins: Vec<String>,
        cap: usize,
    },
    /// `notifications_cleared` push: I read `chat_id` on another device. This Mac drops the
    /// chat's delivered banners, stops showing it unread and re-badges; `badge` is the relay's
    /// unread total after that read.
    NotificationsCleared {
        chat_id: Id,
        badge: u64,
    },
    /// `space_archived` push: another device archived / unarchived a sidebar
    /// space (an agent or a server) for me.
    SpaceArchived {
        space_id: Id,
        archived_at: Option<u64>,
    },
    /// `feedback_capture` push: the user's agent is filing feedback and wants a picture of the
    /// screen; the app answers `POST /feedback/capture/{id}` if it is in the foreground.
    /// The relay's `release_published` push: a new build of `platform` is installable.
    ReleasePublished {
        platform: String,
        version: String,
    },
    FeedbackCapture {
        request_id: String,
        expires_at: Option<i64>,
    },
    /// Secret hand-off: the card's save finished — `Ok(env)` or the error for the shared note.
    SecretHandoff {
        approval_id: String,
        result: Result<String, super::sync::user_error::UserError>,
    },
    /// `GET /voices`: the call voices an agent can use, and the default's id.
    Voices {
        default: Option<String>,
        voices: Vec<Voice>,
    },
    /// A voice sample is on disk (fetched and cached), ready to play.
    VoiceSample {
        url: String,
        path: PathBuf,
    },
    /// Bytes and chat count kept on this Mac (Settings → About).
    StoreUsage {
        bytes: i64,
        chats: i64,
    },
    /// Pairing finished (or failed with a message).
    Paired {
        device_id: String,
    },
    PairingFailed(UserError),
    /// Registry: the full project list, rail order.
    Projects(Vec<Project>),
    /// Model ids the relay proxy can serve (picker contents).
    Models(Vec<String>),
    /// The user's machine (`status.box` at user level).
    Machine(Machine),
    /// A chat room finished its backfill (ROWS_DONE): its doc is current.
    ChatSynced(Id),
    /// The project's workspace on the box (`GET /machine` `workspaces[]` or the
    /// project doc `status.workspace`): running or not.
    Workspace {
        project_id: Id,
        running: bool,
    },
    /// D82: the greeting row exists in this chat (just written, or found).
    GreetingWritten(Id),
    /// Workspace lifecycle for one project (`status.workspace` or relay `STATUS`).
    ProjectStatus {
        project_id: Id,
        state: BoxState,
        detail: Option<String>,
        progress: Option<f32>,
    },
    /// Project doc `chats` index for one project, in tab order.
    Chats {
        project_id: Id,
        chats: Vec<Chat>,
    },
    /// The relay created a chat for a locally minted tab; swap the id.
    ChatCreated {
        project_id: Id,
        local_id: Id,
        chat: Chat,
    },
    /// The project doc: threads (T1 index), status, settings, memory.
    ProjectDoc {
        project_id: Id,
        view: ProjectDocView,
    },
    /// Full message list of one chat doc (after a checkpoint or a list-level edit).
    ChatSnapshot {
        chat_id: Id,
        messages: Vec<Message>,
    },
    /// One message changed (streaming text, status, a new part).
    MessageUpsert {
        chat_id: Id,
        message: Box<Message>,
    },
    /// A blob the user asked for (file card, tool output).
    Blob {
        project_id: Id,
        key: String,
        mime: String,
        bytes: Arc<Vec<u8>>,
        saved_to: Option<PathBuf>,
    },
    /// Bytes of an image attachment for its inline thumbnail (F10); keys are content hashes.
    Thumbnail {
        key: String,
        bytes: Arc<Vec<u8>>,
    },
    /// Picker (ARB-544): a page of GIF results for `query` (empty = trending), or why none.
    GifResults {
        query: String,
        pos: Option<String>,
        page: crate::model::GifPage,
    },
    /// Picker: the relay's sticker packs (`None` when the relay could not be reached).
    StickerPacks {
        packs: Option<Vec<crate::model::StickerPack>>,
    },
    /// The site icon for the named links to `host` (relay `GET /link-icon?domain=`): a
    /// 64×64 PNG, or `None` when the site has none (the bubbles keep the placeholder).
    LinkIcon {
        host: String,
        png: Option<Arc<Vec<u8>>>,
    },
    /// The thumbnail bytes could not be fetched; the card shows "No preview" instead of
    /// loading forever. The failure itself went to the log and the silent diagnostics path.
    ThumbnailFailed {
        key: String,
    },
    /// Copy of a message ([feedback copy-image-messages-mac-2026-10-27]): the original
    /// bytes of every attachment `ClientCommand::FetchCopyBlobs` asked for, in the asked
    /// order; a blob that could not be fetched is `None` in its slot. `request` is the
    /// command's.
    CopyBlobs {
        request: u64,
        blobs: Vec<Option<CopyBlob>>,
    },
    /// An attachment landed in ~/Downloads (F10); the app reveals it.
    AttachmentSaved {
        path: PathBuf,
    },
    /// Play / Quick Look from the media viewer finished fetching `key` (handed
    /// to macOS, or failed with a notice) — the viewer's spinner stops. `path`
    /// is the temp file when the fetch wrote one (videos play from it inline
    /// on macOS; [feedback app-01M6147S9TPNBQA7KSDEY0K8C4]).
    AttachmentOpened {
        key: String,
        path: Option<PathBuf>,
    },
    /// Relay link state (S14), app-wide and coarse.
    Link(Link),
    /// One room's socket state, keyed like `docs` (chat id, or the project id
    /// for the project doc): what the chat header's status pill shows.
    RoomLink {
        key: Id,
        state: LinkState,
    },
    /// What the relay's HTTP side says about reachability (the registry poll):
    /// `Offline` / `RelayDown` while it fails, `Live` as soon as a call succeeds.
    RelayLink(LinkState),
    /// Short user-facing notice — something that worked or a plain status ("Forwarded to
    /// Studio"). Never a failure: those are [`SyncEvent::Failure`].
    Notice(String),
    /// Something the user asked for did not happen. Drawn by `ui::error_note` as an error
    /// toast (or caught by the surface that owns the action); `retry` re-issues the command
    /// behind Try again.
    Failure {
        error: UserError,
        retry: Option<Box<ClientCommand>>,
    },
    /// A room's pushes were refused (`not_available` / `blocked`) or an ACK cleared that
    /// refusal. `refused: true` → the chat-level note draws `why`; `false` → clear it.
    PushRefused {
        chat_id: Id,
        refused: bool,
        why: PushRefusal,
    },
    /// Settings → Connectors: catalog + connected accounts (F1).
    Connectors(ConnectorsView),
    /// An OAuth flow started: open this URL in the browser; the relay finishes the callback.
    ConnectorAuth {
        provider: String,
        auth_url: String,
    },
    /// Settings → Secrets: secrets, accesses, requests, activity (never values; D83).
    Secrets(SecretsView),
    /// Settings → Devices: this account's signed-in devices, or why they could not be read.
    Devices(Result<Vec<DeviceView>, UserError>),
    /// One agent's secrets page (`GET /projects/{id}/secrets`).
    ProjectSecrets {
        project_id: Id,
        view: ProjectSecrets,
    },
    /// One agent's standing shell approvals (#1544).
    ShellGrants {
        project_id: Id,
        items: Vec<crate::model::ShellGrant>,
    },
    /// Routines of one project (Settings → Routines).
    Routines {
        project_id: Id,
        routines: Vec<Routine>,
    },
    /// The box as `GET /machine/debug` reports it (the agent pane's Computer tab).
    BoxReport(BoxReport),
    /// The agent's open background jobs as the relay's activity record lists them
    /// (`GET /projects/{id}` → `activity.jobs[]`, duplex §2.4) — the tasks chip and
    /// drawer. Empty when nothing runs or the relay predates the field.
    ProjectJobs {
        project_id: Id,
        jobs: Vec<crate::model::Job>,
    },
    /// One job's exchange (`GET /projects/{id}/jobs/{jobId}[?after=seq]`) — the rollout
    /// view. `rollout.unavailable` when the relay has no endpoint yet (404).
    JobRollout {
        project_id: Id,
        job_id: String,
        rollout: crate::model::JobRollout,
    },
    /// Run history of one routine, newest first.
    RoutineRuns {
        routine_id: Id,
        runs: Vec<RoutineRun>,
    },
    /// The skills library (F5), whole, after a fetch or a write.
    Skills(Vec<Skill>),
    /// Settings → Channels: platform states, bound chats, active pairing code (F7).
    Channels(ChannelsView),
    /// Settings → This Mac: paired `arbos-local` helpers with online state (F9).
    LocalDevices(LocalView),
    /// Speech-to-text result for `ClientCommand::Transcribe` (F11).
    Transcript {
        request_id: u64,
        text: String,
        error: Option<String>,
        mock: bool,
    },
    /// Text-to-speech result for `ClientCommand::Speak` (F11): encoded audio (WAV by default).
    Speech {
        request_id: u64,
        bytes: Arc<Vec<u8>>,
        mime: String,
        error: Option<String>,
        mock: bool,
    },
    /// D88: whether this relay runs servers at all (`GET /me/matrix` 503 `servers_off` → false).
    ServersAvailable(bool),
    /// D88: the servers this account is in (`GET /servers`), whole.
    Servers(Vec<ServerInfo>),
    /// D88: one server with its members (`GET /servers/{id}`, or after a write).
    ServerDetail(ServerInfo),
    /// D88: a server was just created or joined: select it and land on `#general`.
    ServerOpened(ServerInfo),
    /// D88: a server/channel write failed; shown inline in the open sheet (or as an error
    /// toast when no sheet is up).
    ServerError(UserError),
    /// D88: the room list of servers and channels changed.
    MatrixRooms(Snapshot),
    /// D88: rows of a channel after it was opened, and the whole list after every new row.
    MatrixTimeline {
        room_id: String,
        rows: Vec<MessageRow>,
    },
    /// D88: a `ChannelLoadOlder` found nothing above — the room's history is
    /// whole on this Mac (the Search-in-chat bar stops paging and wraps).
    ChannelHistoryEnd {
        room_id: String,
    },
    /// D88: homeserver connection state.
    MatrixConnection {
        connected: bool,
        error: Option<String>,
    },
    /// D88: a channel send did not go through; the optimistic row is dropped.
    ChannelSendFailed {
        room_id: String,
        local_id: String,
        error: UserError,
    },
    /// A chat send never reached the doc (no room yet, or NewChat failed); mark it failed.
    MessageSendFailed {
        chat_id: Id,
        message_id: Id,
        error: UserError,
    },
}

/// Whose picture the avatar picker edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PictureTarget {
    Agent(Id),
    Group(Id),
    Server(Id),
    /// The Create sheet's not-yet-created agent / group / server (Jacob, 2026-10-25): a picture
    /// generated before the entity exists lands in my own slot (`POST /me/avatar/generate` →
    /// `users:{me}`) and is copied into the new entity's slot on Create.
    Draft,
    /// My own profile picture (the id is my user id; `/me/avatar…` routes).
    Me(Id),
}

impl PictureTarget {
    /// The entity's id; the draft has none yet.
    pub fn id(&self) -> Option<&Id> {
        match self {
            PictureTarget::Agent(id)
            | PictureTarget::Group(id)
            | PictureTarget::Server(id)
            | PictureTarget::Me(id) => Some(id),
            PictureTarget::Draft => None,
        }
    }
}

/// The Create sheet's pending picture, applied once the entity exists (Jacob, 2026-10-25: the
/// sheet offers Upload and Generate only; with neither the entity is stored neutral —
/// `avatar_leaf` = `leaf::NEUTRAL`, the name's initial on a grey disc — never an automatic
/// leaf).
#[derive(Clone, Debug, PartialEq)]
pub enum DraftPicture {
    /// A file the owner chose; uploaded as the picture master (≤ 1024 px PNG / JPEG) into the new entity's slot.
    File(std::path::PathBuf),
    /// A picture the relay already holds in my own slot (`users:{me}`): copied over.
    Generated { key: String },
}

/// Where an agent's new picture comes from (`ClientCommand::SetProjectImage`).
#[derive(Clone, Debug, PartialEq)]
pub enum PictureSource {
    /// A file the owner chose; uploaded as the picture master (≤ 1024 px PNG / JPEG).
    File(std::path::PathBuf),
    /// A blob the relay already stores under the project (a generated picture).
    Key(String),
}

/// One fetched attachment of a Copy (`SyncEvent::CopyBlobs`): the relay's `content-type`
/// and the bytes as uploaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyBlob {
    pub key: String,
    pub mime: String,
    pub bytes: Arc<Vec<u8>>,
}

/// Something the user did; the source writes it into the doc / outbox.
#[derive(Clone, Debug)]
pub enum ClientCommand {
    Pair {
        code: String,
    },
    /// Exchange the one-time grant code from the browser leg (`POST /auth/token`).
    SignIn {
        grant_code: String,
    },
    /// `POST /auth/logout`, forget the tokens, back to the sign-in screen.
    SignOut,
    /// New-chat sheet: `GET /users/search?q=`.
    SearchPeople {
        query: String,
    },
    /// `POST /chats/dm {handle}`; the chat lands in the next `Projects` event.
    StartDm {
        handle: String,
    },
    /// `POST /chats {kind: group, members, agent_ids}` — people, agents, or
    /// both. The chat lands in the next `Projects` event, then
    /// `PeopleGroupCreated` names it.
    CreatePeopleGroup {
        handles: Vec<String>,
        agent_ids: Vec<Id>,
        /// D93 Create screen: the group's name and leaf pair, when it came through it.
        title: Option<String>,
        leaf: Option<crate::ui::avatars::LeafPick>,
        /// The Create screen's uploaded / generated picture, applied right after the create.
        picture: Option<DraftPicture>,
    },
    /// Presence `typing` for a people chat.
    SetTyping {
        chat_id: Id,
        active: bool,
    },
    /// `DELETE /chats/{id}/agents/{project_id}` (owner of that project).
    RemoveAgent {
        chat_id: Id,
        project_id: Id,
    },
    /// Group pane "Add members": `POST /chats/{id}/members {handle}` per person,
    /// `POST /chats/{id}/agents {project_id}` per agent, all on this group.
    AddMembers {
        chat_id: Id,
        handles: Vec<String>,
        agent_ids: Vec<Id>,
    },
    /// `DELETE /chats/{id}/members/{user_id}` (chat owner).
    RemoveMember {
        chat_id: Id,
        user_id: String,
    },
    /// `PUT /chats/{id}/members/{user_id} {"role": "admin" | "member"}` (group owner).
    SetMemberRole {
        chat_id: Id,
        user_id: String,
        role: String,
    },
    /// `POST /chats/{id}/leave`.
    LeaveChat {
        chat_id: Id,
    },
    /// `POST /blocks {handle}`.
    BlockUser {
        handle: String,
    },
    /// `DELETE /blocks/{user_id}`; `handle` only names the person in the notice.
    UnblockUser {
        user_id: String,
        handle: String,
    },
    /// `POST /projects/{id}/members {handle}` (D72 step 3).
    InviteToProject {
        project_id: Id,
        handle: String,
    },
    /// `DELETE /projects/{id}/members/{user_id}`.
    RemoveProjectMember {
        project_id: Id,
        user_id: String,
    },
    /// `PUT /projects/{id}/message-access {grantee, enabled}` — "Who may message X"
    /// (agent-to-agent messaging). `grantee` = `@handle`, `user:<id>` or `project:<id>`.
    SetMessageGrant {
        project_id: Id,
        grantee: String,
        enabled: bool,
    },
    /// `DELETE /projects/{id}/message-access/{grantee}`.
    RemoveMessageGrant {
        project_id: Id,
        grantee: String,
    },
    /// Fetch an avatar once (`SyncEvent::Avatar`).
    FetchAvatar {
        url: String,
    },
    /// Calls v1: `POST /chats/{id}/calls` (or the projects form) — start, or join the live one.
    StartCall {
        chat_id: Id,
        kind: CallKind,
    },
    /// `POST /calls/{id}/join` → a fresh token.
    JoinCall {
        call_id: String,
    },
    /// `POST /calls/{id}/leave` → `{ended, duration_s}`.
    LeaveCall {
        call_id: String,
    },
    /// `GET /calls/{id}` (the "On a call · Join" bar, a push with no detail).
    FetchCall {
        call_id: String,
    },
    /// Write a system line into a chat's doc (call start / end, per the contract).
    SystemLine {
        chat_id: Id,
        text: String,
        call_id: Option<String>,
    },
    /// D82: the agent's greeting in a brand-new project's chat (once per chat).
    Greeting {
        chat_id: Id,
        project_id: Id,
        agent_name: String,
    },
    /// D82: refresh the registry and the machine now (first-login fast lane).
    PollNow,
    OpenChat {
        chat_id: Id,
    },
    Send {
        chat_id: Id,
        message_id: Id,
        text: String,
        attachments: Vec<PathBuf>,
        /// People-chat / hover-reply: the message this send answers (`replyTo`).
        reply_to: Option<String>,
        /// Reply in DM: the message in another chat this send answers, with its
        /// snapshot (`replyRef`). Exclusive with `reply_to`.
        reply_ref: Option<crate::model::ReplyRef>,
        /// Member ids of the composer's committed mention chips (#1596); the
        /// source unions them with the `@handle`s the text names.
        mentions: Vec<String>,
    },
    /// Guide: inject `text` into the agent's currently running turn (`steer` command).
    /// Same message shape as `Send`; the box calls pi/Prime `Steer` instead of queuing.
    Steer {
        chat_id: Id,
        message_id: Id,
        text: String,
        reply_to: Option<String>,
    },
    /// Reply to an `ask_user` question (S8). Same shape as `Send`, different outbox kind.
    /// A typed widget answer carries `question_id` and the structured `value`; `text` is
    /// the rendered form shown in the user's bubble.
    Answer {
        chat_id: Id,
        message_id: Id,
        text: String,
        question_id: Option<String>,
        value: Option<serde_json::Value>,
    },
    Stop {
        chat_id: Id,
    },
    /// The tasks drawer's per-job Stop (duplex D7): `stop { jobId, messageId }` halts
    /// that job only; the running turn and the other jobs go on.
    StopJob {
        chat_id: Id,
        job_id: String,
        /// The newest user message of the chat, or empty — what the box binds the
        /// command to, as the Stop button's `stop` does.
        message_id: String,
    },
    /// Poll the agent's open jobs (`GET /projects/{id}` → `activity.jobs[]`).
    JobsRefresh {
        project_id: Id,
    },
    /// Poll one job's exchange for the open rollout view; `after` asks for the entries
    /// past that `seq` only.
    RolloutRefresh {
        project_id: Id,
        job_id: String,
        after: Option<u64>,
    },
    /// Toggle my emoji reaction on a message (written into the chat doc).
    React {
        chat_id: Id,
        message_id: Id,
        emoji: String,
    },
    /// Delete a message for everyone (removed from the chat doc; #1370).
    DeleteMessage {
        chat_id: Id,
        message_id: Id,
    },
    /// Replace an own message's text in the chat doc (`edit_message_text`).
    EditMessage {
        chat_id: Id,
        message_id: Id,
        text: String,
    },
    /// Remove a message's link preview for everyone (`preview: "off"` on the row, the part
    /// dropped; the relay decides with the delete rule) ([feedback link-embeds-mac-2026-10-27]).
    RemoveLinkPreview {
        chat_id: Id,
        message_id: Id,
    },
    /// S18: decide an approval card (`approve` / `reject` commands, `arbos_core::approval`).
    Approve {
        chat_id: Id,
        approval_id: String,
        /// #1544: card option B/C — the path class to allow from now on (`scratch`,
        /// `workspace`, `anywhere`); `None` is a one-time approve.
        grant: Option<String>,
    },
    Reject {
        chat_id: Id,
        approval_id: String,
        reason: Option<String>,
    },
    Retry {
        chat_id: Id,
        message_id: Id,
    },
    NewChat {
        project_id: Id,
        chat_id: Id,
    },
    /// Session fork (#1257): a new chat of the same project seeded from
    /// `source_chat_id` (`POST /projects/{id}/chats` with `fork_from`); `chat_id`
    /// is the client's placeholder until `ChatCreated` brings the relay's.
    ForkChat {
        project_id: Id,
        source_chat_id: Id,
        chat_id: Id,
        title: String,
    },
    /// Archive (hide from the tab strip) or unarchive a chat for me on every
    /// device (§9c `PUT /chats/{id}/settings {archived}`); the relay keeps it
    /// open, history stays in context, memory and search.
    ArchiveChat {
        chat_id: Id,
        archived: bool,
    },
    /// Replace my sidebar pins (`PUT /me/pins {pins}`): the whole list, top first —
    /// one command for pin, unpin and reorder. The relay's answer comes back as
    /// `SyncEvent::Pins` ([feedback sidebar-pinning-mac-2026-10-28]).
    SetPins {
        pins: Vec<String>,
    },
    /// Rename a chat tab (`PATCH /projects/{id}/chats/{chat}` + a `retitle`
    /// command in the chat doc so the box's local index matches).
    Retitle {
        project_id: Id,
        chat_id: Id,
        title: String,
    },
    MarkRead {
        chat_id: Id,
    },
    /// Name a people group (`PATCH /chats/{id} {title}`), any human member may.
    RenameGroup {
        chat_id: Id,
        title: String,
    },
    /// A group's leaf picture from the avatar picker: `PATCH /chats/{id} {avatar_leaf,
    /// avatar_leaf_color, image_key: null}` (`None` = initials).
    SetGroupLeaf {
        chat_id: Id,
        pick: Option<crate::ui::avatars::LeafPick>,
    },
    /// Give a people group a picture: the file is uploaded to the chat's blob
    /// slot, then `PATCH /chats/{id} {image_key}`.
    SetGroupImage {
        chat_id: Id,
        path: std::path::PathBuf,
    },
    /// D92 amendment: an agent's picture from the avatar picker — a file to upload into the
    /// project's blob slot, or a key the relay already holds (the Generate tab's preview); then
    /// `PATCH /projects/{id} {image_key}`.
    SetProjectImage {
        project_id: Id,
        source: PictureSource,
    },
    /// The avatar picker's Generate tab: `POST /{projects|chats|servers}/{id}/avatar/generate
    /// {prompt}`; the answer arrives as `SyncEvent::ImageGenerated` (not set until "Set avatar").
    GenerateImage {
        target: PictureTarget,
        prompt: String,
    },
    /// A people group's picture from a key the relay already holds (the Generate tab's answer):
    /// `PATCH /chats/{id} {image_key}`.
    SetGroupImageKey {
        chat_id: Id,
        key: String,
    },
    /// A server's picture from the avatar picker (owner / admins): a file uploaded into
    /// `servers:{id}`, or a key the relay already holds; then `PATCH /servers/{id} {image_key}`.
    SetServerImage {
        server_id: Id,
        source: PictureSource,
    },
    /// A server's leaf picture (`None` = initials; clears the uploaded picture too).
    SetServerLeaf {
        server_id: Id,
        pick: Option<crate::ui::avatars::LeafPick>,
    },
    /// My own picture (`PUT /me/avatar`, PNG / JPEG master ≤ 3 MB): overrides the GitHub
    /// avatar everywhere the relay serves me.
    SetMyAvatar {
        path: std::path::PathBuf,
    },
    /// My own picture from a key the relay already holds in my `users:{id}` slot (the avatar
    /// picker's Generate tab): `PUT /me/avatar {image_key}`.
    SetMyAvatarKey {
        key: String,
    },
    /// Back to the GitHub picture (`DELETE /me/avatar`).
    RemoveMyAvatar,
    SetModel {
        project_id: Id,
        model: String,
        effort: Effort,
    },
    /// `PATCH /projects/{id} {"voice": id|null}` (null = current default).
    SetVoice {
        project_id: Id,
        voice: Option<String>,
    },
    /// `PATCH /projects/{id} {"vision_enabled": bool}` — may the agent look
    /// through my camera on calls (voice API §11). Set from the call screen.
    SetVision {
        project_id: Id,
        enabled: bool,
    },
    /// `GET /voices` → [`SyncEvent::Voices`].
    FetchVoices,
    /// Download (once; cached in the config dir) a voice sample → [`SyncEvent::VoiceSample`].
    FetchVoiceSample {
        url: String,
    },
    CreateProject {
        name: String,
        /// D79: the owner's swatch pick at creation; `None` = automatic (the id's colour).
        avatar_color_index: Option<u8>,
        /// D92: the leaf + colour (forks copy the source's; the Create sheet sends the neutral
        /// pair); `None` = automatic.
        leaf: Option<crate::ui::avatars::LeafPick>,
        /// The Create sheet's uploaded / generated picture, applied right after the create.
        picture: Option<DraftPicture>,
        /// Fork: copy this agent's workspace (files + memory) into the new one.
        source_project_id: Option<Id>,
        /// Optional profile blurb for the new agent (forks copy the source's).
        description: Option<String>,
        /// Optional hex accent; forks copy the source's.
        color: Option<String>,
    },
    /// Soft-delete an owned agent (`DELETE /projects/{id}`).
    DeleteProject {
        project_id: Id,
    },
    /// Connect an outside agent (#2141): `POST /outside-agents {name, transport}` →
    /// [`SyncEvent::OutsideAgentKey`] (the key, shown once) and a registry refresh.
    ConnectAgent {
        name: String,
        /// `acp` | `grok` | `custom`.
        transport: String,
        avatar_color_index: Option<u8>,
    },
    /// `GET /outside-agents/{id}` → [`SyncEvent::OutsideAgentStatus`].
    OutsideAgentStatus {
        project_id: Id,
    },
    /// `POST /outside-agents/{id}/rotate` → [`SyncEvent::OutsideAgentKey`] (fresh key).
    OutsideAgentRotate {
        project_id: Id,
    },
    /// `DELETE /outside-agents/{id}` (revoke the key; the agent stays) → status refresh.
    OutsideAgentRevoke {
        project_id: Id,
    },
    /// `PATCH /outside-agents/{id} {scopes: {name: on}}` — one switch of the Can card →
    /// [`SyncEvent::OutsideAgentStatus`] with the effective set.
    OutsideAgentScope {
        project_id: Id,
        scope: String,
        on: bool,
    },
    /// `PATCH /outside-agents/{id} {wake_url, wake_auth}`: `None` leaves a field alone, an
    /// empty string clears it → [`SyncEvent::OutsideAgentWakeSaved`] + a status refresh.
    OutsideAgentWakeSet {
        project_id: Id,
        url: Option<String>,
        auth: Option<String>,
    },
    /// `POST /outside-agents/{id}/wake/test` → [`SyncEvent::OutsideAgentWakeTested`].
    OutsideAgentWakeTest {
        project_id: Id,
    },
    /// Delete a people chat I own (`DELETE /chats/{id}`).
    DeletePeopleChat {
        chat_id: Id,
    },
    /// Close a non-pinned chat tab (`DELETE /projects/{id}/chats/{chat}`).
    CloseChat {
        project_id: Id,
        chat_id: Id,
    },
    MachineStart,
    MachineStop,
    /// Always-on machines: stop then start (`POST /machine/stop`, `/machine/start`).
    MachineRestart,
    /// The chat on screen in a focused window (`None` = none): presence
    /// frames carry it as `focused_chat` so the relay skips pushing to this
    /// device for that chat (API §7c).
    FocusChat {
        chat_id: Option<Id>,
    },
    /// Settings header: `PATCH /me {name, login}` — my display name and handle.
    UpdateProfile {
        name: String,
        handle: String,
    },
    /// Agent page: `PATCH /projects/{id} {description}` (API §9; `""` clears).
    SetDescription {
        project_id: Id,
        description: String,
    },
    /// Rename an agent: `PATCH /projects/{id} {name}` (API §9).
    SetProjectName {
        project_id: Id,
        name: String,
    },
    /// Profile pages: `PUT /chats/{id}/settings {muted}` for a people chat (syncs
    /// across devices); projects keep the local mute list.
    SetMuted {
        space_id: Id,
        muted: bool,
    },
    /// Sidebar archive (2026-10-23): `PUT /projects/{id}/archive` or
    /// `PUT /servers/{id}/archive` `{archived}` — hides the row for me on every
    /// device; nothing is deleted. People chats use `ArchiveChat`.
    ArchiveSpace {
        space_id: Id,
        server: bool,
        archived: bool,
    },
    /// Settings → Notifications: send and show Presence `{seen}` read receipts
    /// in people chats (default on). Local preference; presence frames omit
    /// `seen` when off. When `persist` is true (the Settings toggle), also
    /// `PUT /chats/{id}/settings {read_receipts}` for every people chat.
    SetReadReceipts {
        enabled: bool,
        persist: bool,
    },
    /// Leaf picker (D92): `PATCH /projects/{id} {avatar_leaf: 0–8|null, avatar_leaf_color: 0–9|null}`;
    /// `None` = automatic (derived from the project id).
    SetAvatarLeaf {
        project_id: Id,
        leaf: Option<u8>,
        color: Option<u8>,
    },
    FetchBlob {
        project_id: Id,
        key: String,
        save: bool,
    },
    /// Fetch an image attachment's bytes for the inline thumbnail (F10). Keys that start
    /// with `url:` (a picked GIF) or `sticker:` (a relay-hosted sticker) are fetched
    /// through the relay's picker endpoints instead of `/blob` (ARB-544).
    FetchThumbnail {
        project_id: Id,
        key: String,
    },
    /// Picker (ARB-544): one tap sends a sticker or a GIF as its own message.
    SendMedia {
        chat_id: Id,
        message_id: Id,
        pick: crate::model::MediaPick,
        reply_to: Option<String>,
    },
    /// Picker: search the GIF provider through the relay (`q` empty = trending).
    GifSearch {
        query: String,
        pos: Option<String>,
    },
    /// Picker: the relay's sticker-pack manifest.
    StickerPacks,
    /// Picker: tell the provider a GIF was sent (ranking signal; never an error).
    GifShare {
        id: String,
        query: String,
    },
    /// Copy of a message: the full original bytes of every attachment in `keys` (blob
    /// slot `project_id`), answered together as `SyncEvent::CopyBlobs { request, .. }`
    /// ([feedback copy-image-messages-mac-2026-10-27]). Never a thumbnail: the
    /// pasteboard gets what the sender uploaded.
    FetchCopyBlobs {
        project_id: Id,
        request: u64,
        keys: Vec<String>,
    },
    /// Fetch the site icon the bubbles draw before named links to `host`.
    FetchLinkIcon {
        host: String,
    },
    /// Download an attachment to `~/Downloads/<name>` (F10).
    DownloadAttachment {
        project_id: Id,
        key: String,
        name: String,
    },
    /// Media viewer: fetch `key` to a temp file. Files open in Quick Look
    /// (`qlmanage -p`); videos play inline in the viewer on macOS (elsewhere
    /// the default app) — [feedback app-01M6147S9TPNBQA7KSDEY0K8C4].
    OpenAttachment {
        project_id: Id,
        key: String,
        name: String,
        quick_look: bool,
    },
    /// Media viewer → Forward: copy the blob `attachment.blob_ref` from
    /// `from_slot` into `chat_id`'s slot and write a user row carrying it plus
    /// `text` ("Forwarded from …"). `message_id` is the optimistic row's id.
    Forward {
        chat_id: Id,
        message_id: Id,
        from_slot: Id,
        attachment: Attachment,
        text: String,
    },
    /// Bubble menu → Forward: write the picked message again as a new user row
    /// in `chat_id` — its text, every attachment (each blob copied from
    /// `from_slot` into the destination's slot, so the target chat's members can
    /// read it) and every chart part with its fallback picture — carrying
    /// `forwardedFrom: <name>` (the bubble's "Forwarded from" header).
    /// `message_id` is the optimistic row's id
    /// ([feedback forward-attachments-mac-2026-10-27]).
    ForwardMessage {
        chat_id: Id,
        message_id: Id,
        from_slot: Id,
        text: String,
        attachments: Vec<Attachment>,
        charts: Vec<ChartBody>,
        forwarded_from: String,
    },
    /// Settings → Connectors opened or asked for a refresh.
    RefreshConnectors,
    /// Connect a provider: OAuth when `token` is `None`, else store the pasted token.
    ConnectorConnect {
        provider: String,
        token: Option<String>,
    },
    ConnectorDisconnect {
        id: Id,
    },
    /// Settings → Secrets opened or asked for a refresh.
    SecretsRefresh,
    /// Settings → Devices opened or asked for a refresh (`GET /auth/devices`).
    DevicesRefresh,
    /// Settings → Devices: sign another device out (`DELETE /auth/devices/{id}`).
    DeviceRevoke {
        id: Id,
    },
    /// An agent's Secrets page opened.
    ProjectSecretsRefresh {
        project_id: Id,
    },
    /// The agent pane opened: its standing shell approvals (#1544).
    ShellGrantsRefresh {
        project_id: Id,
    },
    /// Settings › Shell approvals: switch a grant off; the agent asks again next time.
    ShellGrantRevoke {
        project_id: Id,
        grant_id: String,
    },
    /// Secret hand-off request card: the value goes from this device to `POST /secrets`
    /// (source `chat`, the agent granted), then the card is answered with the saved secret's
    /// name and id — never the value — so the agent learns only the name.
    SecretHandoffSave {
        chat_id: Id,
        approval_id: String,
        project_id: Id,
        name: String,
        value: String,
        purpose: String,
    },
    /// Add a secret; the value goes to the relay once.
    SecretAdd {
        item: NewSecret,
    },
    /// `PATCH /secrets/{id}` (name, note, hosts, ask_every_time, expires_at).
    SecretPatch {
        id: Id,
        body: serde_json::Value,
    },
    /// `PUT /secrets/{id}/value`: a new version; the old one works for 24 h.
    SecretReplaceValue {
        id: Id,
        body: serde_json::Value,
    },
    SecretDelete {
        id: Id,
    },
    /// One Access switch (`PUT /secrets/{id}/access/{project_id}`).
    SecretAccess {
        id: Id,
        project_id: Id,
        enabled: bool,
        note: Option<String>,
    },
    /// Every Access of a secret off.
    SecretRevokeAll {
        id: Id,
    },
    /// One People row (`PUT /secrets/{id}/people/{user_id}`): switch and note (spec §7.7).
    SecretPerson {
        id: Id,
        user_id: Id,
        enabled: bool,
        note: Option<String>,
    },
    /// Add people from the picker (`POST /secrets/{id}/people { handle }`, one per handle).
    SecretPeopleAdd {
        id: Id,
        handles: Vec<String>,
    },
    /// Take a person off the list (`DELETE /secrets/{id}/people/{user_id}`).
    SecretPersonRemove {
        id: Id,
        user_id: Id,
    },
    /// The kill switch (`PUT /me/secrets/settings`).
    SecretsPause {
        paused: bool,
    },
    /// Answer a request: `once | chat | always | deny`.
    SecretDecide {
        request_id: Id,
        decision: String,
    },
    /// Connect a 1Password vault (Phase 2); the token goes to the relay once.
    VaultConnect {
        token: String,
        agents: Vec<Id>,
        note: String,
    },
    VaultAccess {
        id: Id,
        project_id: Id,
        enabled: bool,
    },
    VaultDisconnect {
        id: Id,
    },
    /// `PUT /vaults/{id}/items/env`: one item's real value into (or out of) the agents' environment.
    VaultItemEnv {
        id: Id,
        reference: String,
        on: bool,
    },
    /// `PUT /vaults/{id}/token`: a new service-account token (validated by the relay first).
    VaultReplaceToken {
        id: Id,
        token: String,
    },
    /// Settings → Routines opened or needs a refresh.
    RoutinesRefresh {
        project_id: Id,
    },
    RoutineCreate {
        project_id: Id,
        name: String,
        prompt: String,
        chat_id: Id,
        trigger: RoutineTrigger,
    },
    /// Pause (`false`) or resume (`true`).
    RoutineSetEnabled {
        project_id: Id,
        routine_id: Id,
        enabled: bool,
    },
    RoutineRunNow {
        project_id: Id,
        routine_id: Id,
    },
    RoutineRuns {
        project_id: Id,
        routine_id: Id,
    },
    RoutineDelete {
        project_id: Id,
        routine_id: Id,
    },
    /// F5: refetch the skills library.
    SkillsRefresh,
    /// F5: create (`id` empty) or update a skill; the source answers with `SyncEvent::Skills`.
    SkillSave {
        skill: Skill,
    },
    SkillDelete {
        id: Id,
    },
    /// Settings → Channels opened or asked for a refresh.
    RefreshChannels,
    /// Mint a pairing code for a project (its pinned chat unless `chat_id`).
    ChannelPairNew {
        project_id: Id,
        chat_id: Option<Id>,
    },
    ChannelUnbind {
        id: Id,
    },
    /// Settings → This Mac opened or needs a refresh.
    LocalRefresh,
    /// The agent pane opened (or its Computer tab was picked): fetch `GET /machine/debug`.
    MachineDebugRefresh,
    /// Per-device "Allow reads without asking".
    LocalSetAllowReads {
        device_id: Id,
        allow: bool,
    },
    /// Forget a helper (and revoke its device token).
    LocalForget {
        device_id: Id,
    },
    /// F11: a spoken message. `wav` (16 kHz mono PCM) is uploaded to the blob
    /// store and kept on the row as an `attachment` part `{ kind: "audio" }`.
    SendVoice {
        chat_id: Id,
        message_id: Id,
        text: String,
        wav: Arc<Vec<u8>>,
    },
    /// F11: speech-to-text through the relay proxy; answered by `SyncEvent::Transcript`.
    Transcribe {
        project_id: Id,
        request_id: u64,
        wav: Arc<Vec<u8>>,
    },
    /// F11: text-to-speech through the relay proxy; answered by `SyncEvent::Speech`.
    Speak {
        project_id: Id,
        request_id: u64,
        text: String,
    },
    /// D88: `GET /servers` again (after a push or when the list is shown).
    ServersRefresh,
    /// D88: `POST /servers {name}`; answered by `ServerOpened`.
    ServerCreate {
        name: String,
        /// D93 Create screen: the server's leaf pair (the neutral pair since 2026-10-25).
        leaf: Option<crate::ui::avatars::LeafPick>,
        /// The Create screen's uploaded / generated picture, applied right after the create.
        picture: Option<DraftPicture>,
    },
    /// D88: `POST /servers/join {code}` with a code or a full invite link; `ServerOpened`.
    ServerJoin {
        code: String,
    },
    /// D88: `GET /servers/{id}` (members); answered by `ServerDetail`.
    ServerFetch {
        server_id: Id,
    },
    /// D88: `PATCH /servers/{id} {name}` (owner/admin).
    ServerRename {
        server_id: Id,
        name: String,
    },
    /// D88: `DELETE /servers/{id}` (owner).
    ServerDelete {
        server_id: Id,
    },
    /// D88: `POST /servers/{id}/leave`.
    ServerLeave {
        server_id: Id,
    },
    /// D88: `POST /servers/{id}/invite/reset` (owner).
    /// D88: `POST /servers/{id}/invites {expires, max_uses}` — the owner mints a link.
    ServerInviteCreate {
        server_id: Id,
        expires: bool,
        max_uses: Option<i64>,
    },
    /// D88: `DELETE /servers/{id}/invites/{iid}` — the owner revokes a link.
    ServerInviteRevoke {
        server_id: Id,
        invite_id: Id,
    },

    /// D88: `PUT /servers/{id}/members/{uid} {role}` (owner); `admin` or `member`.
    ServerSetRole {
        server_id: Id,
        user_id: Id,
        role: String,
    },
    /// D88: `DELETE /servers/{id}/members/{uid}[?ban=1]` (owner/admin).
    ServerRemoveMember {
        server_id: Id,
        user_id: Id,
        ban: bool,
    },
    /// D88: `POST /servers/{id}/channels {name, restricted}` (owner/admin).
    ChannelCreate {
        server_id: Id,
        name: String,
        restricted: bool,
    },
    /// D88: `PATCH /servers/{id}/channels/{cid}` (owner/admin).
    ChannelPatch {
        server_id: Id,
        channel_id: Id,
        name: Option<String>,
        topic: Option<String>,
        restricted: Option<bool>,
    },
    /// D88: `DELETE /servers/{id}/channels/{cid}` (owner/admin; `#general` refuses).
    ChannelDelete {
        server_id: Id,
        channel_id: Id,
    },
    /// D88: `POST /servers/{id}/members {user_id}` per person — owner/admin adds people by name.
    ServerAddMembers {
        server_id: Id,
        user_ids: Vec<Id>,
    },
    /// D88: `POST /servers/{id}/channels/{cid}/archive {archived}` — hide the channel from my
    /// list or bring it back (per user, mirrored as the room's `m.lowpriority` tag).
    ChannelArchive {
        server_id: Id,
        channel_id: Id,
        archived: bool,
    },
    /// D88: load a channel's rows (first page + live); answered by `MatrixTimeline`.
    ChannelOpen {
        room_id: String,
    },
    /// D88: page older rows in; answered by `MatrixTimeline` when there were any.
    ChannelLoadOlder {
        room_id: String,
    },
    /// D88: send a text row; the echo carries `local_id` and replaces the optimistic row.
    ChannelSend {
        room_id: String,
        local_id: String,
        text: String,
    },
    /// D88: read receipt up to the newest row.
    ChannelMarkRead {
        room_id: String,
    },
}

/// Collective mentions (Jacob, 2026-10-24, after #1596): `@here` is everyone
/// in the chat — its people (not the sender) and its agents — and `@agents`
/// every agent. Any member may use them in a group or a people chat. The sent
/// row carries the expansion as member ids, which is what every agent's gate
/// reads (`people_gate` rule 3, a resolved `agent:<me>` mention; two or more
/// agent ids make it a shared reply), so the agents wake.
pub const COLLECTIVE_HERE: &str = "here";
pub const COLLECTIVE_AGENTS: &str = "agents";

/// `here` / `agents` — a collective handle (the composer's token id for its
/// chip is the handle itself, a marker the sources expand).
pub fn is_collective(handle: &str) -> bool {
    handle.eq_ignore_ascii_case(COLLECTIVE_HERE) || handle.eq_ignore_ascii_case(COLLECTIVE_AGENTS)
}

/// `@token` as a whole word in `lower` (lower-case text).
pub fn text_has_at_token(lower: &str, token: &str) -> bool {
    let needle = format!("@{token}");
    lower.match_indices(&needle).any(|(i, _)| {
        let before = lower[..i].chars().last();
        let after = lower[i + needle.len()..].chars().next();
        before.is_none_or(|c| !c.is_alphanumeric() && c != '@')
            && after.is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '-' || c == '.'))
    })
}

/// The member ids a send starts from: the composer's chips (`tokens`: member
/// ids and collective markers) and the `@here` / `@agents` the text names,
/// collectives expanded into the chat's members; markers never reach the row.
pub fn expand_collectives(
    space: &Project,
    text_lower: &str,
    tokens: &[String],
    me_id: &str,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |id: String| {
        if !out.contains(&id) {
            out.push(id);
        }
    };
    let here = tokens.iter().any(|t| t == COLLECTIVE_HERE)
        || text_has_at_token(text_lower, COLLECTIVE_HERE);
    let agents = tokens.iter().any(|t| t == COLLECTIVE_AGENTS)
        || text_has_at_token(text_lower, COLLECTIVE_AGENTS);
    for t in tokens {
        if !is_collective(t) {
            push(t.clone());
        }
    }
    if here {
        for p in &space.people {
            if p.id != me_id {
                push(format!("user:{}", p.id));
            }
        }
    }
    if here || agents {
        for a in &space.agents {
            push(format!("agent:{}", a.project_id));
        }
    }
    out
}

/// The handles (and name slugs) the composer's mention chips already account
/// for (#1596): the text scan skips these so a chip for one of two entities
/// sharing a handle (#1538) is the one and only answer for that handle.
pub fn claimed_handles(space: &Project, tokens: &[String]) -> HashSet<String> {
    let mut out = HashSet::new();
    for t in tokens {
        if let Some(pid) = t.strip_prefix("agent:") {
            for a in space.agents.iter().filter(|a| a.project_id == pid) {
                out.insert(a.handle.to_lowercase());
                let slug = Project::mention_slug(&a.name);
                if !slug.is_empty() {
                    out.insert(slug);
                }
            }
        } else if let Some(uid) = t.strip_prefix("user:") {
            for p in space.people.iter().filter(|p| p.id == uid) {
                out.insert(p.handle.to_lowercase());
            }
        }
    }
    out
}

pub trait SyncSource: Send + 'static {
    /// Short name shown in the settings popover ("fixture", "relay").
    fn name(&self) -> &'static str;

    /// Run until the command channel closes. The future runs on tokio.
    fn run(
        self: Box<Self>,
        events: mpsc::UnboundedSender<SyncEvent>,
        commands: mpsc::UnboundedReceiver<ClientCommand>,
    ) -> BoxFuture<'static, anyhow::Result<()>>;
}

/// The relay everyone talks to unless told otherwise.
pub const DEFAULT_RELAY_URL: &str = "https://relay.arbos.life";

/// Pick the source. Order: `ARBOS_RELAY_URL` (`fixture` runs the seeded demo
/// source), then `defaults read sh.arbos.mac RelayURL` for self-hosters on
/// macOS, then the compiled-in default relay.
pub fn source_from_env() -> Box<dyn SyncSource> {
    let url = configured_relay_url();
    if url.trim().eq_ignore_ascii_case("fixture") {
        return Box::new(fixture::FixtureSource::new());
    }
    Box::new(relay::RelaySource::from_env(url))
}

/// The relay URL this process talks to (same order as `source_from_env`); other
/// modules use it to find the stored credentials for one-off HTTP calls.
pub fn configured_relay_url() -> String {
    match std::env::var("ARBOS_RELAY_URL") {
        Ok(url) if !url.trim().is_empty() => url,
        Ok(_) | Err(_) => {
            relay_url_from_defaults().unwrap_or_else(|| DEFAULT_RELAY_URL.to_string())
        }
    }
}

/// `defaults write sh.arbos.mac RelayURL https://relay.example` (macOS only).
fn relay_url_from_defaults() -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = std::process::Command::new("defaults")
        .args(["read", "sh.arbos.mac", "RelayURL"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let url = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!url.is_empty()).then_some(url)
}

#[cfg(test)]
mod collective_tests {
    use super::*;
    use crate::model::{AgentMember, Person};

    fn space() -> Project {
        Project {
            id: "g-launch".into(),
            name: "Launch week".into(),
            people: vec![
                Person {
                    id: "gh_me".into(),
                    handle: "jacob".into(),
                    ..Default::default()
                },
                Person {
                    id: "gh_sam".into(),
                    handle: "sam".into(),
                    ..Default::default()
                },
                Person {
                    id: "gh_mira".into(),
                    handle: "mira-k".into(),
                    ..Default::default()
                },
            ],
            agents: vec![
                AgentMember {
                    project_id: "p-comms".into(),
                    name: "Comms".into(),
                    handle: "comms".into(),
                    ..Default::default()
                },
                AgentMember {
                    project_id: "p-test".into(),
                    name: "TestAgent2".into(),
                    handle: "testagent2".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn here_is_everyone_but_me_and_agents_is_every_agent() {
        let s = space();
        // A chip (marker) — the row carries the members, never the marker.
        let ids = expand_collectives(
            &s,
            "how is the iphone now @here",
            &["here".to_string()],
            "gh_me",
        );
        assert_eq!(
            ids,
            vec![
                "user:gh_sam",
                "user:gh_mira",
                "agent:p-comms",
                "agent:p-test"
            ]
        );
        assert!(!ids.iter().any(|i| i == "here"));
        // Plain text (no chip) expands the same way.
        assert_eq!(
            expand_collectives(&s, "@agents status?", &[], "gh_me"),
            vec!["agent:p-comms", "agent:p-test"]
        );
        // Member chips stay in front, deduplicated against the expansion.
        assert_eq!(
            expand_collectives(
                &s,
                "@comms and @agents",
                &["agent:p-comms".to_string(), "agents".to_string()],
                "gh_me"
            ),
            vec!["agent:p-comms", "agent:p-test"]
        );
        // Nothing collective → nothing added.
        assert_eq!(
            expand_collectives(&s, "hello there", &[], "gh_me"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn at_token_is_a_whole_word() {
        assert!(text_has_at_token("ping @here now", "here"));
        assert!(text_has_at_token("@agents?", "agents"));
        assert!(!text_has_at_token("@heresy", "here"));
        assert!(!text_has_at_token("mail a@here.com", "here"));
        assert!(!text_has_at_token("@agents-2", "agents"));
        assert!(is_collective("Here") && is_collective("agents") && !is_collective("sam"));
    }
}

#[cfg(test)]
mod push_refusal_tests {
    //! The chat-level note's words for a refused push ([feedback blocked-state-relay-2026-10-26]).
    use super::{muted_until_label, PushRefusal};
    use chrono::{FixedOffset, TimeZone};

    fn blocked(reason: &str, name: Option<&str>) -> PushRefusal {
        PushRefusal {
            code: "not_available".into(),
            reason: Some(reason.into()),
            by_handle: Some("sam".into()),
            by_name: name.map(str::to_owned),
        }
    }

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).expect("UTC")
    }

    #[test]
    fn muted_until_label_shapes() {
        let tz = utc();
        let now = tz
            .with_ymd_and_hms(2026, 10, 28, 12, 0, 0)
            .single()
            .expect("now")
            .timestamp_millis();
        let later_today = tz
            .with_ymd_and_hms(2026, 10, 28, 14, 30, 0)
            .single()
            .expect("later")
            .timestamp_millis();
        let other_day = tz
            .with_ymd_and_hms(2026, 10, 30, 14, 30, 0)
            .single()
            .expect("other")
            .timestamp_millis();
        assert_eq!(
            muted_until_label(Some(later_today), now, tz),
            "Muted until 14:30"
        );
        assert_eq!(
            muted_until_label(Some(other_day), now, tz),
            "Muted until Oct 30, 14:30"
        );
        assert_eq!(muted_until_label(None, now, tz), "Muted");
    }

    #[test]
    fn the_sentence_names_who_stands_in_the_way() {
        assert_eq!(
            blocked("blocked_by", Some("Sam Rivera")).sentence(None),
            "Sam Rivera has blocked you in this chat."
        );
        assert_eq!(
            blocked("blocked_by", None).sentence(None),
            "@sam has blocked you in this chat."
        );
        assert_eq!(
            blocked("blocks_member", Some("Sam Rivera")).sentence(None),
            "Couldn't post: you've blocked Sam Rivera, who is in this chat. Unblock to post."
        );
        let tz = utc();
        let now = tz
            .with_ymd_and_hms(2026, 10, 28, 12, 0, 0)
            .single()
            .expect("now")
            .timestamp_millis();
        let until = tz
            .with_ymd_and_hms(2026, 10, 28, 14, 30, 0)
            .single()
            .expect("until")
            .timestamp_millis();
        assert_eq!(
            PushRefusal {
                code: "not_available".into(),
                reason: Some("silenced".into()),
                ..Default::default()
            }
            .sentence_at(Some(until), now, tz),
            "Muted until 14:30. Your messages aren't being delivered."
        );
        assert_eq!(
            PushRefusal {
                code: "not_available".into(),
                reason: Some("silenced".into()),
                ..Default::default()
            }
            .sentence_at(None, now, tz),
            "Muted. Your messages aren't being delivered."
        );
    }

    #[test]
    fn an_old_relay_without_a_reason_keeps_the_generic_line() {
        for why in [
            PushRefusal::code("not_available"),
            PushRefusal {
                code: "not_available".into(),
                reason: Some("chat_unavailable".into()),
                ..Default::default()
            },
        ] {
            assert_eq!(
                why.sentence(None),
                "Messages in this chat aren't being delivered right now. We'll keep trying."
            );
        }
        assert_eq!(
            PushRefusal {
                reason: Some("blocked_by".into()),
                by_handle: Some(String::new()),
                ..Default::default()
            }
            .sentence(None),
            "Someone in this chat has blocked you."
        );
    }

    #[test]
    fn a_timed_mute_clears_once_the_clock_passes_the_end() {
        use crate::model::Project;
        use std::collections::HashMap;

        let mut project = Project {
            id: "g1".into(),
            silenced: true,
            silenced_until: Some(1_000),
            ..Project::default()
        };
        let mut refused = HashMap::from([(
            "g1".into(),
            PushRefusal {
                code: "not_available".into(),
                reason: Some("silenced".into()),
                ..Default::default()
            },
        )]);
        assert!(!super::clear_silence_if_expired(
            &mut project,
            &mut refused,
            999
        ));
        assert!(project.silenced);
        assert!(refused.contains_key("g1"));
        assert!(super::clear_silence_if_expired(
            &mut project,
            &mut refused,
            1_000
        ));
        assert!(!project.silenced);
        assert!(project.silenced_until.is_none());
        assert!(!refused.contains_key("g1"));
    }
}
