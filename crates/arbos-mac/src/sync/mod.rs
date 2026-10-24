//! The data layer. A [`SyncSource`] feeds the UI with [`SyncEvent`]s and takes
//! [`ClientCommand`]s back. Two sources exist: [`fixture::FixtureSource`]
//! (local Loro docs, scripted agent) and [`relay::RelaySource`] (HTTP + one
//! WebSocket per room on `arbos-relay`, `crdt-sync.md` §7–§9).
//!
//! Everything in this module is GPUI-free. Wire frames and the doc schema
//! constants come from `arbos-core`; the client-side mirror (`doc.rs`) and
//! the sources are candidates to move there later (`arbos-design.md` §7.2).

pub mod credentials;
pub mod diagnostics;
pub mod doc;
pub mod dto;
pub mod fixture;
pub mod relay;
pub mod store;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use arbos_matrix::{MessageRow, Snapshot};
use futures::future::BoxFuture;
use tokio::sync::mpsc;

use crate::model::{
    AccountView, Call, CallEnded, CallKind, CallMissed, Person, Reachable, Sender, ServerInfo,
    Voice,
};
use crate::model::{
    Attachment, BoxState, ChannelsView, Chat, ConnectorsView, DeviceView, Effort, Id, Link,
    LinkState, LocalView, Machine, Message, NewSecret, Project, ProjectDocView, ProjectSecrets,
    Routine, RoutineRun, RoutineTrigger, SecretsView, Skill,
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
    /// The avatar picker's generated picture is ready to preview (`url` fetches it; `image_key`
    /// is what "Set avatar" sends), or the generation failed (`error`).
    ProjectImageGenerated {
        project_id: Id,
        image_key: Option<String>,
        url: Option<String>,
        error: Option<String>,
    },
    /// Error from a people-chat action (search, DM, members), shown inline.
    PeopleError(String),
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
    /// The thumbnail bytes could not be fetched; the card shows "No preview" instead of
    /// loading forever. The failure itself went to the log and the silent diagnostics path.
    ThumbnailFailed {
        key: String,
    },
    /// An attachment landed in ~/Downloads (F10); the app reveals it.
    AttachmentSaved {
        path: PathBuf,
    },
    /// Play / Quick Look from the media viewer finished fetching `key` (handed
    /// to macOS, or failed with a notice) — the viewer's spinner stops.
    AttachmentOpened {
        key: String,
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
    /// Settings → Devices: this account's signed-in devices, or why they could not be read.
    Devices(Result<Vec<DeviceView>, String>),
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
    /// D88: whether this relay runs servers at all (`GET /me/matrix` 503 `servers_off` → false).
    ServersAvailable(bool),
    /// D88: the servers this account is in (`GET /servers`), whole.
    Servers(Vec<ServerInfo>),
    /// D88: one server with its members (`GET /servers/{id}`, or after a write).
    ServerDetail(ServerInfo),
    /// D88: a server was just created or joined: select it and land on `#general`.
    ServerOpened(ServerInfo),
    /// D88: a server/channel write failed; shown inline in the open sheet and as a notice.
    ServerError(String),
    /// D88: the room list of servers and channels changed.
    MatrixRooms(Snapshot),
    /// D88: rows of a channel after it was opened, and the whole list after every new row.
    MatrixTimeline {
        room_id: String,
        rows: Vec<MessageRow>,
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
        error: String,
    },
    /// A chat send never reached the doc (no room yet, or NewChat failed); mark it failed.
    MessageSendFailed {
        chat_id: Id,
        message_id: Id,
        error: String,
    },
}

/// Where an agent's new picture comes from (`ClientCommand::SetProjectImage`).
#[derive(Clone, Debug, PartialEq)]
pub enum PictureSource {
    /// A file the owner chose; uploaded as a 512 px JPEG.
    File(std::path::PathBuf),
    /// A blob the relay already stores under the project (a generated picture).
    Key(String),
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
        /// D93 Create screen: the group's name and leaf picture, when it came through it.
        title: Option<String>,
        leaf: Option<crate::ui::avatars::LeafPick>,
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
    /// The avatar picker's Generate tab: `POST /projects/{id}/avatar/generate {prompt}`; the
    /// answer arrives as `SyncEvent::ProjectImageGenerated` (not set until "Set avatar").
    GenerateProjectImage {
        project_id: Id,
        prompt: String,
    },
    /// My own picture (`PUT /me/avatar`, JPEG ≤ 2 MB): overrides the GitHub
    /// avatar everywhere the relay serves me.
    SetMyAvatar {
        path: std::path::PathBuf,
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
        /// D92: the leaf + colour the Create sheet previewed (forks copy the source's); `None` = automatic.
        leaf: Option<crate::ui::avatars::LeafPick>,
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
    /// Media viewer: fetch `key` to a temp file and hand it to macOS — Quick
    /// Look (`qlmanage -p`) for files, the default app (QuickTime Player) for video.
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
        /// D93 Create screen: the server's leaf picture.
        leaf: Option<crate::ui::avatars::LeafPick>,
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
