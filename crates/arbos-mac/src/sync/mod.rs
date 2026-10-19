//! The data layer. A [`SyncSource`] feeds the UI with [`SyncEvent`]s and takes
//! [`ClientCommand`]s back. Two sources exist: [`fixture::FixtureSource`]
//! (local Loro docs, scripted agent) and [`relay::RelaySource`] (HTTP + one
//! WebSocket per room on `arbos-relay`, `crdt-sync.md` §7–§9).
//!
//! Everything in this module is GPUI-free. Wire frames and the doc schema
//! constants come from `arbos-core`; the client-side mirror (`doc.rs`) and
//! the sources are candidates to move there later (`arbos-design.md` §7.2).

pub mod credentials;
pub mod doc;
pub mod fixture;
pub mod relay;
pub mod store;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use futures::future::BoxFuture;
use tokio::sync::mpsc;

use crate::model::{
    AccountView, Call, CallEnded, CallKind, CallMissed, Person, Reachable, Sender, Voice,
};
use crate::model::{
    BoxState, ChannelsView, Chat, ConnectorsView, Effort, Id, Link, LocalView, Machine, Message,
    NewSecret, Project, ProjectDocView, ProjectSecrets, Routine, RoutineRun, RoutineTrigger,
    SecretsView, Skill,
};

/// Something the source learned; the UI applies it to its mirror.
#[derive(Clone, Debug)]
pub enum SyncEvent {
    /// No device token for this relay yet: show the pairing screen.
    NeedsPairing {
        relay_url: String,
    },
    /// Who is signed in (`/auth/token` or `/me`); `None` after sign-out.
    Account(Option<AccountView>),
    /// `GET /users/search` results for the new-chat sheet (D72).
    PeopleSearch {
        query: String,
        results: Vec<Person>,
    },
    /// Someone in a people chat started/stopped typing (room presence).
    /// Someone's device sent a presence frame in a room we share (API §7c
    /// frames carry `handle`): they are online right now.
    Presence {
        handle: String,
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
    /// Error from a people-chat action (search, DM, members), shown inline.
    PeopleError(String),
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
    /// A call request failed; shown as a notice and the call UI resets.
    CallError {
        chat_id: Id,
        message: String,
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
    /// `feedback_capture` push: the user's agent is filing feedback and wants a picture of the
    /// screen; the app answers `POST /feedback/capture/{id}` if it is in the foreground.
    FeedbackCapture {
        request_id: String,
        expires_at: Option<i64>,
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
    PairingFailed(String),
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
    /// An attachment landed in ~/Downloads (F10); the app reveals it.
    AttachmentSaved {
        path: PathBuf,
    },
    /// Relay link state (S14).
    Link(Link),
    /// Short user-facing notice (errors from HTTP calls, etc.).
    Notice(String),
    /// Settings → Connectors: catalog + connected accounts (F1).
    Connectors(ConnectorsView),
    /// An OAuth flow started: open this URL in the browser; the relay finishes the callback.
    ConnectorAuth {
        provider: String,
        auth_url: String,
    },
    /// Settings → Secrets: secrets, accesses, requests, activity (never values; D83).
    Secrets(SecretsView),
    /// One agent's secrets page (`GET /projects/{id}/secrets`).
    ProjectSecrets {
        project_id: Id,
        view: ProjectSecrets,
    },
    /// Routines of one project (Settings → Routines).
    Routines {
        project_id: Id,
        routines: Vec<Routine>,
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
    /// Presence `typing` for a people chat.
    SetTyping {
        chat_id: Id,
        active: bool,
    },
    /// `POST /chats/{id}/agents {project_id}` (own project only).
    AddAgent {
        chat_id: Id,
        project_id: Id,
    },
    /// `DELETE /chats/{id}/agents/{project_id}` (owner of that project).
    RemoveAgent {
        chat_id: Id,
        project_id: Id,
    },
    /// `DELETE /chats/{id}/members/{user_id}` (chat owner).
    RemoveMember {
        chat_id: Id,
        user_id: String,
    },
    /// `POST /chats/{id}/leave`.
    LeaveChat {
        chat_id: Id,
    },
    /// `POST /blocks {handle}`.
    BlockUser {
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
    /// Toggle my emoji reaction on a message (written into the chat doc).
    React {
        chat_id: Id,
        message_id: Id,
        emoji: String,
    },
    /// S18: decide an approval card (`approve` / `reject` commands, `arbos_core::approval`).
    Approve {
        chat_id: Id,
        approval_id: String,
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
    /// F6: a group chat hosted by `project_id` with the other `participants`; `chat_id` is the
    /// client's placeholder until `ChatCreated` brings the relay's.
    GroupCreate {
        project_id: Id,
        chat_id: Id,
        participants: Vec<Id>,
        title: Option<String>,
    },
    GroupAddParticipant {
        project_id: Id,
        chat_id: Id,
        participant: Id,
    },
    /// Archive (hide from the tab strip) or unarchive a chat for me on every
    /// device (§9c `PUT /chats/{id}/settings {archived}`); the relay keeps it
    /// open, history stays in context, memory and search.
    ArchiveChat {
        chat_id: Id,
        archived: bool,
    },
    MarkRead {
        chat_id: Id,
    },
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
    /// `GET /voices` → [`SyncEvent::Voices`].
    FetchVoices,
    /// Voice API §11: `PATCH /projects/{id} {"vision_enabled": bool}`.
    SetVisionEnabled {
        project_id: Id,
        enabled: bool,
    },
    /// Download (once; cached in the config dir) a voice sample → [`SyncEvent::VoiceSample`].
    FetchVoiceSample {
        url: String,
    },
    CreateProject {
        name: String,
        /// D79: the owner's swatch pick at creation; `None` = automatic (the id's colour).
        avatar_color_index: Option<u8>,
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
    /// Avatar picker (D79): `PATCH /projects/{id} {avatar_color_index: 0–6|null}`;
    /// `None` = automatic (derived from the project id).
    SetAvatarColorIndex {
        project_id: Id,
        index: Option<u8>,
    },
    FetchBlob {
        project_id: Id,
        key: String,
        save: bool,
    },
    /// Fetch an image attachment's bytes for the inline thumbnail (F10).
    FetchThumbnail {
        project_id: Id,
        key: String,
    },
    /// Download an attachment to `~/Downloads/<name>` (F10).
    DownloadAttachment {
        project_id: Id,
        key: String,
        name: String,
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
    /// An agent's Secrets page opened.
    ProjectSecretsRefresh {
        project_id: Id,
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
