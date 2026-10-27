//! UniFFI surface for the Swift client (arbos-client + arbos-core + arbos-matrix).
//!
//! This is the only crate that names Swift or `uniffi`. It wraps
//! [`arbos_client::RoomSession`] and the parts of `arbos-core` the Swift app
//! needs. Core and client stay free of UniFFI derives.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use arbos_client::room::{
    self as client_room, RoomAction as ClientAction, RoomSession as ClientSession,
};
use arbos_core::doc::{self, decode_vv, encode_vv, ChatDoc as CoreChatDoc};
use arbos_core::wire::{self, Frame, FrameType as WireFrameType};
use loro::Subscription;
use serde_json::Value;

uniffi::setup_scaffolding!();

pub mod avatar;
pub mod chart;
pub mod highlight;
pub mod matrix;
pub mod search;
pub mod settings_search;

/// Frame decode, doc import, or writer failure. Sans-IO session has no transport errors.
#[derive(Debug, Clone, thiserror::Error, uniffi::Error)]
pub enum FfiError {
    #[error("{0}")]
    Decode(String),
    #[error("{0}")]
    Import(String),
    #[error("{0}")]
    Write(String),
}

fn write_err(e: impl ToString) -> FfiError {
    FfiError::Write(e.to_string())
}

/// Pending PUSH batch (owned; replaces `(String, Vec<u8>)` over the FFI).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PendingPush {
    pub batch_id: String,
    pub bytes: Vec<u8>,
}

/// Mirrors [`arbos_core::wire::BoxState`] for UniFFI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum BoxState {
    Awake,
    Waking,
    Asleep,
    Error,
}

impl From<wire::BoxState> for BoxState {
    fn from(s: wire::BoxState) -> Self {
        match s {
            wire::BoxState::Awake => Self::Awake,
            wire::BoxState::Waking => Self::Waking,
            wire::BoxState::Asleep => Self::Asleep,
            wire::BoxState::Error => Self::Error,
        }
    }
}

/// Owned host actions from [`RoomSession::drain_actions`].
#[derive(Debug, Clone, uniffi::Enum)]
pub enum RoomAction {
    SendFrame {
        bytes: Vec<u8>,
    },
    FetchCheckpoint {
        room: String,
        frontier: Vec<u8>,
        checkpoint_seq: i64,
    },
    ApplyBytes {
        bytes: Vec<u8>,
    },
    Cursor {
        seq: i64,
    },
    Pending {
        batches: Vec<PendingPush>,
    },
    BoxState {
        state: BoxState,
        detail: Option<String>,
        poll_machine: bool,
    },
    LinkLive,
    /// Export the doc's updates since `relay_vv` and always hand them to `on_heal`
    /// (empty when nothing is missing) so the held pending resend can proceed.
    HealCheck {
        relay_vv: Vec<u8>,
    },
    ScheduleReconnect {
        after_ms: u64,
    },
    SessionError {
        code: String,
        message: String,
    },
    /// A `not_available` push refusal on this connection (`batchId` from the error) with the
    /// relay's `reason` (`blocked_by` / `blocks_member` / `silenced` / `chat_unavailable`)
    /// and, on a block, the other party.
    PushRefused {
        batch_id: Option<String>,
        code: String,
        reason: Option<String>,
        by_handle: Option<String>,
        by_name: Option<String>,
    },
    Presence {
        device: Option<String>,
        handle: Option<String>,
        typing: Option<bool>,
        in_call: Option<String>,
        mine: bool,
        /// Full inbound PRESENCE header JSON (Swift `notePresence` reads `user` / `at`).
        header_json: String,
    },
    DroppedOversize {
        bytes: u64,
    },
    Ack {
        batch_id: String,
        seq: i64,
    },
}

impl From<ClientAction> for RoomAction {
    fn from(a: ClientAction) -> Self {
        match a {
            ClientAction::SendFrame(bytes) => Self::SendFrame { bytes },
            ClientAction::FetchCheckpoint {
                room,
                frontier,
                checkpoint_seq,
            } => Self::FetchCheckpoint {
                room,
                frontier,
                checkpoint_seq,
            },
            ClientAction::ApplyBytes(bytes) => Self::ApplyBytes { bytes },
            ClientAction::Cursor(seq) => Self::Cursor { seq },
            ClientAction::Pending(batches) => Self::Pending {
                batches: batches
                    .into_iter()
                    .map(|(batch_id, bytes)| PendingPush { batch_id, bytes })
                    .collect(),
            },
            ClientAction::BoxState {
                state,
                detail,
                poll_machine,
            } => Self::BoxState {
                state: state.into(),
                detail,
                poll_machine,
            },
            ClientAction::LinkLive => Self::LinkLive,
            ClientAction::HealCheck { relay_vv } => Self::HealCheck { relay_vv },
            ClientAction::ScheduleReconnect { after } => Self::ScheduleReconnect {
                after_ms: after.as_millis() as u64,
            },
            ClientAction::SessionError { code, message } => Self::SessionError { code, message },
            ClientAction::PushRefused {
                batch_id,
                code,
                reason,
                by_handle,
                by_name,
            } => Self::PushRefused {
                batch_id,
                code,
                reason,
                by_handle,
                by_name,
            },
            ClientAction::Presence {
                device,
                handle,
                typing,
                in_call,
                mine,
                header,
            } => Self::Presence {
                device,
                handle,
                typing,
                in_call,
                mine,
                header_json: serde_json::to_string(&header).unwrap_or_else(|_| "{}".into()),
            },
            ClientAction::DroppedOversize { bytes } => Self::DroppedOversize {
                bytes: bytes as u64,
            },
            ClientAction::Ack { batch_id, seq } => Self::Ack { batch_id, seq },
        }
    }
}

/// Wire frame type for [`encode_frame`] / [`decode_frame`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FrameType {
    Hello,
    State,
    RowsReq,
    Row,
    RowsDone,
    Push,
    Ack,
    Presence,
    Probe,
    ProbeOk,
    Error,
    Status,
    Wake,
}

impl From<WireFrameType> for FrameType {
    fn from(t: WireFrameType) -> Self {
        match t {
            WireFrameType::Hello => Self::Hello,
            WireFrameType::State => Self::State,
            WireFrameType::RowsReq => Self::RowsReq,
            WireFrameType::Row => Self::Row,
            WireFrameType::RowsDone => Self::RowsDone,
            WireFrameType::Push => Self::Push,
            WireFrameType::Ack => Self::Ack,
            WireFrameType::Presence => Self::Presence,
            WireFrameType::Probe => Self::Probe,
            WireFrameType::ProbeOk => Self::ProbeOk,
            WireFrameType::Error => Self::Error,
            WireFrameType::Status => Self::Status,
            WireFrameType::Wake => Self::Wake,
        }
    }
}

impl From<FrameType> for WireFrameType {
    fn from(t: FrameType) -> Self {
        match t {
            FrameType::Hello => Self::Hello,
            FrameType::State => Self::State,
            FrameType::RowsReq => Self::RowsReq,
            FrameType::Row => Self::Row,
            FrameType::RowsDone => Self::RowsDone,
            FrameType::Push => Self::Push,
            FrameType::Ack => Self::Ack,
            FrameType::Presence => Self::Presence,
            FrameType::Probe => Self::Probe,
            FrameType::ProbeOk => Self::ProbeOk,
            FrameType::Error => Self::Error,
            FrameType::Status => Self::Status,
            FrameType::Wake => Self::Wake,
        }
    }
}

/// One decoded room frame (header as JSON text).
#[derive(Debug, Clone, uniffi::Record)]
pub struct DecodedFrame {
    pub ty: FrameType,
    pub header_json: String,
    pub payload: Vec<u8>,
}

/// Encode one binary room frame. `header_json` must be a JSON object.
#[uniffi::export]
pub fn encode_frame(
    ty: FrameType,
    header_json: String,
    payload: Vec<u8>,
) -> Result<Vec<u8>, FfiError> {
    let header: Value =
        serde_json::from_str(&header_json).map_err(|e| FfiError::Decode(e.to_string()))?;
    if !header.is_object() {
        return Err(FfiError::Decode("header must be a JSON object".into()));
    }
    Ok(Frame::new(ty.into(), header, payload).encode().to_vec())
}

/// Decode one binary room frame. Unknown type → [`FfiError::Decode`].
#[uniffi::export]
pub fn decode_frame(bytes: Vec<u8>) -> Result<DecodedFrame, FfiError> {
    match Frame::decode(bytes.into()) {
        Ok(Some(frame)) => {
            let header_json = serde_json::to_string(&frame.header)
                .map_err(|e| FfiError::Decode(e.to_string()))?;
            Ok(DecodedFrame {
                ty: frame.ty.into(),
                header_json,
                payload: frame.payload.to_vec(),
            })
        }
        Ok(None) => Err(FfiError::Decode("unknown frame type".into())),
        Err(e) => Err(FfiError::Decode(e.to_string())),
    }
}

/// Sans-IO room session for Swift: `Send + Sync` via interior `Mutex`.
#[derive(uniffi::Object)]
pub struct RoomSession {
    inner: Mutex<ClientSession>,
}

impl RoomSession {
    /// Recover a poisoned lock instead of panicking across the FFI.
    fn lock(&self) -> MutexGuard<'_, ClientSession> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[uniffi::export]
impl RoomSession {
    #[uniffi::constructor]
    pub fn new(device_id: String, room: String, cursor: i64, pending: Vec<PendingPush>) -> Self {
        let pending = pending.into_iter().map(|p| (p.batch_id, p.bytes)).collect();
        Self {
            inner: Mutex::new(ClientSession::new(device_id, room, cursor, pending)),
        }
    }

    pub fn room(&self) -> String {
        self.lock().room().to_string()
    }

    pub fn cursor(&self) -> i64 {
        self.lock().cursor()
    }

    pub fn pending(&self) -> Vec<PendingPush> {
        self.lock()
            .pending()
            .iter()
            .map(|(batch_id, bytes)| PendingPush {
                batch_id: batch_id.clone(),
                bytes: bytes.clone(),
            })
            .collect()
    }

    pub fn backoff_ms(&self) -> u64 {
        self.lock().backoff().as_millis() as u64
    }

    pub fn reconnect_after_ms(&self) -> u64 {
        self.lock().reconnect_after().as_millis() as u64
    }

    pub fn is_live(&self) -> bool {
        self.lock().is_live()
    }

    pub fn is_closed(&self) -> bool {
        self.lock().is_closed()
    }

    pub fn drain_actions(&self) -> Vec<RoomAction> {
        self.lock()
            .drain_actions()
            .into_iter()
            .map(RoomAction::from)
            .collect()
    }

    pub fn on_open(&self, now_ms: u64) {
        self.lock().on_open(now_ms);
    }

    pub fn on_frame(&self, bytes: Vec<u8>) {
        self.lock().on_frame(&bytes);
    }

    pub fn on_local_commit(&self, bytes: Vec<u8>) {
        self.lock().on_local_commit(bytes);
    }

    /// Answer to `RoomAction::HealCheck`: the doc's updates the relay lacked.
    /// Call even when empty so the session can release the held pending resend.
    pub fn on_heal(&self, bytes: Vec<u8>) {
        self.lock().on_heal(bytes);
    }

    pub fn on_tick(&self, now_ms: u64) {
        self.lock().on_tick(now_ms);
    }

    pub fn on_close(&self, now_ms: u64) {
        self.lock().on_close(now_ms);
    }

    pub fn on_checkpoint(&self, seq: i64) {
        self.lock().on_checkpoint(seq);
    }

    pub fn on_checkpoint_skipped(&self) {
        self.lock().on_checkpoint_skipped();
    }

    pub fn on_checkpoint_failed(&self) {
        self.lock().on_checkpoint_failed();
    }

    /// Host failed to import row bytes; reconnect from the last good cursor.
    /// The import of the last `ApplyBytes` was parked by Loro (dependencies missing): the
    /// persisted cursor stays below that row; a corrective `Cursor` action follows.
    pub fn apply_pending(&self) {
        self.lock().apply_pending();
    }

    pub fn apply_failed(&self) {
        self.lock().apply_failed();
    }

    pub fn prepare_reconnect(&self) {
        self.lock().prepare_reconnect();
    }

    pub fn wake(&self, reason: String) {
        self.lock().wake(reason);
    }

    /// `presence_header` is a JSON object string (open-ended: typing, in_call, …).
    pub fn presence(&self, presence_header: String) {
        let header = parse_presence_header(&presence_header);
        self.lock().presence(header);
    }

    /// Same header shape as [`Self::presence`]; sets / clears the `typing` field.
    pub fn typing(&self, presence_header: String, active: bool) {
        let header = parse_presence_header(&presence_header);
        self.lock().typing(header, active);
    }
}

fn parse_presence_header(s: &str) -> Value {
    match serde_json::from_str::<Value>(s) {
        Ok(Value::Object(map)) => Value::Object(map),
        Ok(_) | Err(_) => Value::Object(serde_json::Map::new()),
    }
}

/// Change notification matching Swift `ChatDoc.Touched`.
#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct Touched {
    pub list_changed: bool,
    pub messages: Vec<u64>,
    pub other: bool,
}

/// One attachment on a client user message (maps to core `UserAttachment`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct UserAttachmentRecord {
    pub name: String,
    pub mime: String,
    pub kind: Option<String>,
    pub blob_ref: Option<String>,
}

impl From<UserAttachmentRecord> for doc::UserAttachment {
    fn from(r: UserAttachmentRecord) -> Self {
        Self {
            name: r.name,
            mime: r.mime,
            kind: r.kind,
            blob_ref: r.blob_ref,
        }
    }
}

/// Options for [`ChatDoc::insert_user_message`] (maps to core `UserMessageOpts`).
#[derive(Debug, Clone, PartialEq, Eq, Default, uniffi::Record)]
pub struct UserMessageOptsRecord {
    pub attachments: Vec<UserAttachmentRecord>,
    pub mentions: Vec<String>,
    pub reply_to: Option<String>,
    pub sender: Option<String>,
    pub sender_handle: Option<String>,
    pub sender_name: Option<String>,
    pub sender_avatar: Option<String>,
    /// Bubble menu → Forward: the original sender's display name, stored on the
    /// row as `forwardedFrom` (the "Forwarded from <name>" header).
    #[uniffi(default = None)]
    pub forwarded_from: Option<String>,
    /// Bubble menu → Reply in DM: where the quoted message lives plus its
    /// snapshot, stored on the row as `replyRef` (core `ReplyRef`).
    #[uniffi(default = None)]
    pub reply_ref: Option<ReplyRefRecord>,
}

/// A cross-chat reply reference (maps to core `doc::ReplyRef`).
#[derive(Debug, Clone, PartialEq, Eq, Default, uniffi::Record)]
pub struct ReplyRefRecord {
    pub chat_id: String,
    /// `"people"` for DMs / people groups, else the project id.
    pub project_id: String,
    pub message_id: String,
    pub chat_title: String,
    pub sender: String,
    pub sender_name: String,
    pub sender_handle: String,
    /// The quoted prose; the Rust side cuts it to the snapshot length.
    pub text: String,
}

impl From<ReplyRefRecord> for doc::ReplyRef {
    fn from(r: ReplyRefRecord) -> Self {
        Self {
            chat_id: r.chat_id,
            project_id: r.project_id,
            message_id: r.message_id,
            chat_title: r.chat_title,
            sender: r.sender,
            sender_name: r.sender_name,
            sender_handle: r.sender_handle,
            text: doc::ReplyRef::snapshot_text(&r.text),
        }
    }
}

impl From<UserMessageOptsRecord> for doc::UserMessageOpts {
    fn from(r: UserMessageOptsRecord) -> Self {
        Self {
            attachments: r.attachments.into_iter().map(Into::into).collect(),
            mentions: r.mentions,
            reply_to: r.reply_to,
            sender: r.sender,
            sender_handle: r.sender_handle,
            sender_name: r.sender_name,
            sender_avatar: r.sender_avatar,
            answers_question_id: None,
            // The phone's zone, found on the Rust side so the Swift record is unchanged.
            tz: doc::device_time_zone(),
        }
    }
}

/// Whether `sender` currently holds `emoji` on a message (flat key or legacy bucket).
fn reaction_is_on(doc: &CoreChatDoc, message_id: &str, emoji: &str, sender: &str) -> bool {
    let Some(map) = doc.find_message(message_id) else {
        return false;
    };
    let mut json = serde_json::to_value(map.get_deep_value()).unwrap_or(Value::Null);
    doc::project_reactions(&mut json);
    json.get("reactions")
        .and_then(|r| r.get(emoji))
        .and_then(|bucket| bucket.get(sender))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Foreign callback: Swift implements this instead of `ChatDoc.onChange`.
///
/// The callback runs on the caller's thread (the thread that called
/// [`ChatDoc::apply`], [`ChatDoc::snapshot`], [`ChatDoc::export_updates_since`],
/// or [`ChatDoc::commit`]), after the doc lock is released.
#[uniffi::export(foreign)]
pub trait DocChangeListener: Send + Sync {
    fn on_change(&self, touched: Touched);
}

/// Foreign callback: local commit blobs (same role as
/// `CoreChatDoc.onLocalUpdate`).
///
/// Delivered on the caller's thread after the doc lock is released, and
/// before any [`DocChangeListener`] events from the same flush.
#[uniffi::export(foreign)]
pub trait LocalUpdateListener: Send + Sync {
    fn on_local_update(&self, update: Vec<u8>);
}

/// Chat doc wrapper: core `ChatDoc` is not `Sync` (`Cell` txn depth), so `Mutex`.
#[derive(uniffi::Object)]
pub struct ChatDoc {
    inner: Mutex<CoreChatDoc>,
    subs: Mutex<Vec<Subscription>>,
    /// Touched events queued while the doc lock is held; flushed after drop.
    pending: Arc<Mutex<Vec<Touched>>>,
    listener: Arc<Mutex<Option<Arc<dyn DocChangeListener>>>>,
    /// Local update blobs queued while the doc lock is held; flushed after drop.
    pending_local_updates: Arc<Mutex<Vec<Vec<u8>>>>,
    local_update_listener: Arc<Mutex<Option<Arc<dyn LocalUpdateListener>>>>,
}

impl ChatDoc {
    /// Recover a poisoned doc lock instead of panicking across the FFI.
    fn lock_doc(&self) -> MutexGuard<'_, CoreChatDoc> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_subs(&self) -> MutexGuard<'_, Vec<Subscription>> {
        self.subs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_pending(&self) -> MutexGuard<'_, Vec<Touched>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_listener(&self) -> MutexGuard<'_, Option<Arc<dyn DocChangeListener>>> {
        self.listener.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_pending_local_updates(&self) -> MutexGuard<'_, Vec<Vec<u8>>> {
        self.pending_local_updates
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_local_update_listener(&self) -> MutexGuard<'_, Option<Arc<dyn LocalUpdateListener>>> {
        self.local_update_listener
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Deliver queued local updates then change notifications after the doc
    /// lock is dropped. Local updates first so a host that sends from its
    /// change handler already has the update blob.
    fn flush_pending(&self) {
        let queued_updates = std::mem::take(&mut *self.lock_pending_local_updates());
        let queued = std::mem::take(&mut *self.lock_pending());
        let local_listener = self.lock_local_update_listener().clone();
        let listener = self.lock_listener().clone();
        if let Some(local_listener) = local_listener {
            for update in queued_updates {
                local_listener.on_local_update(update);
            }
        }
        if let Some(listener) = listener {
            for touched in queued {
                listener.on_change(touched);
            }
        }
    }

    fn new_wrapped(doc: CoreChatDoc) -> Self {
        Self {
            inner: Mutex::new(doc),
            subs: Mutex::new(Vec::new()),
            pending: Arc::new(Mutex::new(Vec::new())),
            listener: Arc::new(Mutex::new(None)),
            pending_local_updates: Arc::new(Mutex::new(Vec::new())),
            local_update_listener: Arc::new(Mutex::new(None)),
        }
    }
}

#[uniffi::export]
impl ChatDoc {
    /// Empty replica (random Loro peer, matching Mac/iOS client behaviour).
    #[uniffi::constructor]
    pub fn open() -> Self {
        Self::new_wrapped(CoreChatDoc::new(None, false))
    }

    /// Import a full snapshot into a new replica.
    #[uniffi::constructor]
    pub fn open_from_snapshot(snapshot: Vec<u8>) -> Result<Self, FfiError> {
        let doc = CoreChatDoc::from_snapshot(None, false, &snapshot)
            .map_err(|e| FfiError::Import(e.to_string()))?;
        Ok(Self::new_wrapped(doc))
    }

    /// Import update or snapshot bytes.
    /// Import an update; `true` when Loro parked it as pending (its dependencies are not in
    /// the doc yet), `false` when it applied. Hosts report `true` with `RoomSession.apply_pending`.
    pub fn apply_update(&self, bytes: Vec<u8>) -> Result<bool, FfiError> {
        let result = {
            self.lock_doc()
                .import(&bytes)
                .map(|status| status.pending.is_some())
                .map_err(|e| FfiError::Import(e.to_string()))
        };
        self.flush_pending();
        result
    }

    pub fn apply(&self, bytes: Vec<u8>) -> Result<(), FfiError> {
        let result = {
            self.lock_doc()
                .import(&bytes)
                .map(|_| ())
                .map_err(|e| FfiError::Import(e.to_string()))
        };
        self.flush_pending();
        result
    }

    /// Updates since an encoded version vector (`oplog_version` / frontier bytes).
    pub fn export_updates_since(&self, from_vv: Vec<u8>) -> Vec<u8> {
        let vv = decode_vv(&from_vv).unwrap_or_default();
        let out = self.lock_doc().export_updates_since(&vv);
        self.flush_pending();
        out
    }

    /// Ops this replica holds beyond an encoded version vector (the relay's `ROWS_DONE`
    /// payload); empty when none — the history-heal payload.
    pub fn updates_beyond(&self, theirs_vv: Vec<u8>) -> Vec<u8> {
        let Some(vv) = decode_vv(&theirs_vv) else {
            return Vec::new();
        };
        let out = self.lock_doc().updates_beyond(&vv);
        self.flush_pending();
        out
    }

    pub fn snapshot(&self) -> Vec<u8> {
        let out = self.lock_doc().snapshot();
        self.flush_pending();
        out
    }

    /// Encoded oplog version vector (frontier bytes).
    pub fn oplog_version(&self) -> Vec<u8> {
        let vv = self.lock_doc().oplog_vv();
        encode_vv(&vv)
    }

    pub fn contains_frontier(&self, frontier: Vec<u8>) -> bool {
        self.lock_doc().contains_frontier(&frontier)
    }

    /// Message rows as a JSON array, with `project_hidden_parts` and
    /// `project_reactions` already applied.
    pub fn read_rows_json(&self) -> String {
        let rows = self.lock_doc().projected_rows();
        serde_json::to_string(&rows).unwrap_or_else(|_| "[]".into())
    }

    /// Number of rows in the `messages` list.
    pub fn message_count(&self) -> u64 {
        self.lock_doc().messages_list().len() as u64
    }

    /// One projected message row (`project_hidden_parts` + `project_reactions`),
    /// same shape as an element of [`Self::read_rows_json`].
    pub fn read_message_json(&self, index: u64) -> Option<String> {
        let row = self.lock_doc().projected_row(index as usize)?;
        Some(serde_json::to_string(&row).unwrap_or_else(|_| "null".into()))
    }

    /// JSON object for a root map (`settings`, `meta`, …).
    pub fn read_map_json(&self, name: String) -> String {
        let doc = self.lock_doc();
        let map = doc.doc.get_map(name.as_str());
        let json =
            serde_json::to_value(map.get_deep_value()).unwrap_or(Value::Object(Default::default()));
        serde_json::to_string(&json).unwrap_or_else(|_| "{}".into())
    }

    /// True when the replica has no oplog ops yet (nothing imported or written).
    pub fn is_empty(&self) -> bool {
        self.lock_doc().doc.oplog_frontiers().is_empty()
    }

    /// Append a user message. Does not commit.
    pub fn insert_user_message(
        &self,
        id: String,
        text: String,
        device_id: String,
        opts: UserMessageOptsRecord,
    ) -> Result<(), FfiError> {
        let forwarded_from = opts.forwarded_from.clone();
        let reply_ref = opts.reply_ref.clone().map(doc::ReplyRef::from);
        let core_opts = doc::UserMessageOpts::from(opts);
        let doc = self.lock_doc();
        doc.append_user_message(&id, &text, &device_id, &core_opts)
            .map_err(write_err)?;
        if let Some(by) = forwarded_from.as_deref().filter(|b| !b.trim().is_empty()) {
            doc.set_message_fields(&id, &[("forwardedFrom", Value::String(by.to_owned()))]);
        }
        if let Some(r) = reply_ref.filter(|r| !r.chat_id.is_empty() && !r.message_id.is_empty()) {
            doc.set_reply_ref(&id, &r);
        }
        Ok(())
    }

    /// Append a system line. Does not commit. `link_id` is core's optional `callId`.
    pub fn insert_system_line(
        &self,
        id: String,
        text: String,
        device_id: String,
        link_id: Option<String>,
    ) -> Result<(), FfiError> {
        self.lock_doc()
            .append_system_line(&id, &text, &device_id, link_id.as_deref())
            .map_err(write_err)
    }

    /// Append a D82 assistant greeting. Does not commit.
    pub fn insert_assistant_greeting(
        &self,
        id: String,
        text: String,
        device_id: String,
        project_id: String,
        agent_name: String,
    ) -> Result<(), FfiError> {
        self.lock_doc()
            .append_greeting(&id, &text, &device_id, &project_id, &agent_name)
            .map_err(write_err)
    }

    /// Toggle one emoji reaction for `sender`. Does not commit.
    ///
    /// Reads current state through [`doc::project_reactions`] (flat
    /// `"emoji|sender"` or legacy bucket), then calls core `set_reaction`
    /// with the opposite.
    pub fn toggle_reaction(
        &self,
        message_id: String,
        emoji: String,
        sender: String,
    ) -> Result<(), FfiError> {
        let doc = self.lock_doc();
        let on = reaction_is_on(&doc, &message_id, &emoji, &sender);
        doc.set_reaction(&message_id, &emoji, &sender, !on)
            .map_err(write_err)
    }

    /// Remove one message from the shared list (delete for everyone). Does not
    /// commit. Callers must not announce — peers learn via room sync only.
    pub fn delete_message(&self, message_id: String) -> Result<(), FfiError> {
        self.lock_doc()
            .delete_message(&message_id)
            .map_err(write_err)
    }

    /// Replace the text of an existing message (edit). Does not commit. Sets
    /// `editedAt`. No-op when the message or its text part is missing.
    pub fn edit_message(&self, message_id: String, text: String) -> Result<(), FfiError> {
        self.lock_doc()
            .edit_message_text(&message_id, &text)
            .map_err(write_err)
    }

    /// Append a `commands` row. Does not commit. Returns the new command id.
    /// `payload_json` must be a JSON object.
    pub fn add_command(
        &self,
        kind: String,
        payload_json: String,
        device_id: String,
    ) -> Result<String, FfiError> {
        let payload: Value = serde_json::from_str(&payload_json).map_err(write_err)?;
        if !payload.is_object() {
            return Err(FfiError::Write("payload_json must be a JSON object".into()));
        }
        Ok(self.lock_doc().push_command(&kind, payload, &device_id))
    }

    /// Mirror the model picker into project `settings`. Does not commit.
    pub fn set_model_settings(&self, model: String, effort: String) -> Result<(), FfiError> {
        self.lock_doc()
            .set_model_settings(&model, &effort)
            .map_err(write_err)
    }

    /// Commit pending writes, then deliver queued change events after the lock
    /// is released (same 5a rule as [`Self::apply`]).
    pub fn commit(&self) {
        {
            self.lock_doc().doc.commit();
        }
        self.flush_pending();
    }

    /// Subscribe to root diffs; maps through core [`doc::touched`].
    ///
    /// Events are queued while the doc lock is held and delivered on the
    /// caller's thread after [`Self::apply`] / [`Self::snapshot`] /
    /// [`Self::export_updates_since`] / [`Self::commit`] release the guard
    /// (so a Swift listener that calls [`Self::read_rows_json`] does not
    /// deadlock).
    pub fn on_change(&self, listener: Arc<dyn DocChangeListener>) {
        *self.lock_listener() = Some(listener);
        let pending = self.pending.clone();
        let sub = {
            let doc = self.lock_doc();
            doc.doc.subscribe_root(Arc::new(move |event| {
                let t = doc::touched(&event);
                pending
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(Touched {
                        list_changed: t.list_changed,
                        messages: t.messages.iter().map(|i| *i as u64).collect(),
                        other: t.other,
                    });
            }))
        };
        self.lock_subs().push(sub);
    }

    /// Subscribe to local commit update blobs (not remote [`Self::apply`] imports).
    ///
    /// Loro invokes the handler inside commit while the doc lock is held, so
    /// updates are queued and delivered in [`Self::flush_pending`] after the
    /// guard drops — same rule as [`Self::on_change`].
    pub fn on_local_update(&self, listener: Arc<dyn LocalUpdateListener>) {
        *self.lock_local_update_listener() = Some(listener);
        let pending = self.pending_local_updates.clone();
        let sub = {
            let doc = self.lock_doc();
            doc.doc.subscribe_local_update(Box::new(move |bytes| {
                pending
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(bytes.to_vec());
                true
            }))
        };
        self.lock_subs().push(sub);
    }
}

/// Room path helper (people vs project), same as `arbos_client::room_path`.
#[uniffi::export]
pub fn room_path(project_id: String, chat_id: String, is_people: bool) -> String {
    client_room::room_path(&project_id, &chat_id, is_people)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn state_frame(checkpoint_seq: i64) -> Vec<u8> {
        let header = json!({
            "headSeq": 10,
            "seqFloor": 0,
            "checkpointSeq": checkpoint_seq,
            "checkpointSize": 0,
            "rowCount": 0,
            "rowBytes": 0,
            "box": "awake",
        });
        Frame::new(WireFrameType::State, header, Vec::new())
            .encode()
            .to_vec()
    }

    #[test]
    fn room_session_smoke_on_open_and_state() {
        let s = RoomSession::new("dev:me".into(), "proj/chat".into(), 0, Vec::new());
        s.on_open(1_000);
        let open = s.drain_actions();
        assert!(matches!(open.as_slice(), [RoomAction::SendFrame { .. }]));
        let hello = decode_frame(match &open[0] {
            RoomAction::SendFrame { bytes } => bytes.clone(),
            _ => unreachable!(),
        })
        .unwrap();
        assert_eq!(hello.ty, FrameType::Hello);

        s.on_frame(state_frame(0));
        let after = s.drain_actions();
        assert!(after.iter().any(|a| matches!(
            a,
            RoomAction::BoxState {
                state: BoxState::Awake,
                poll_machine: false,
                ..
            }
        )));
        assert!(s.is_live());
        assert!(after.iter().any(|a| match a {
            RoomAction::SendFrame { bytes } => {
                matches!(decode_frame(bytes.clone()), Ok(f) if f.ty == FrameType::RowsReq)
            }
            _ => false,
        }));
    }

    #[test]
    fn chat_doc_round_trip_snapshot() {
        let doc = ChatDoc::open();
        {
            let guard = doc.lock_doc();
            guard.transact(|d| {
                d.append_user_message(
                    "m1",
                    "hi",
                    "dev",
                    &arbos_core::doc::UserMessageOpts::default(),
                )
                .unwrap();
            });
        }
        let snap = doc.snapshot();
        let loaded = ChatDoc::open_from_snapshot(snap).unwrap();
        let rows: Value = serde_json::from_str(&loaded.read_rows_json()).unwrap();
        assert_eq!(rows[0]["role"], "user");
        assert_eq!(rows[0]["parts"][0]["text"], "hi");
    }

    struct RecordingListener {
        rows: Mutex<Vec<usize>>,
        doc: Arc<ChatDoc>,
    }

    impl DocChangeListener for RecordingListener {
        fn on_change(&self, _touched: Touched) {
            // Re-enter the doc on the same thread; must not deadlock.
            let json = self.doc.read_rows_json();
            let parsed: Value = serde_json::from_str(&json).unwrap();
            self.rows
                .lock()
                .unwrap()
                .push(parsed.as_array().map(|a| a.len()).unwrap_or(0));
        }
    }

    #[test]
    fn change_callback_does_not_deadlock_on_reentrant_read() {
        let doc = Arc::new(ChatDoc::open());
        let listener = Arc::new(RecordingListener {
            rows: Mutex::new(Vec::new()),
            doc: doc.clone(),
        });
        doc.on_change(listener.clone());

        {
            let guard = doc.lock_doc();
            guard.transact(|d| {
                d.append_user_message(
                    "m1",
                    "hi",
                    "dev",
                    &arbos_core::doc::UserMessageOpts::default(),
                )
                .unwrap();
            });
        }
        // snapshot commits and emits; flush runs after the lock drops.
        let _ = doc.snapshot();
        assert!(
            !listener.rows.lock().unwrap().is_empty(),
            "listener should have fired after snapshot"
        );
    }

    #[test]
    fn insert_user_message_reads_back_attachment_mentions_reply() {
        let doc = ChatDoc::open();
        assert!(doc.is_empty());
        doc.insert_user_message(
            "m1".into(),
            "hello".into(),
            "dev:1".into(),
            UserMessageOptsRecord {
                attachments: vec![UserAttachmentRecord {
                    name: "photo.jpg".into(),
                    mime: "image/jpeg".into(),
                    kind: Some("image".into()),
                    blob_ref: Some("blob:1".into()),
                }],
                mentions: vec!["user:a".into(), "user:b".into()],
                reply_to: Some("m0".into()),
                sender: Some("user:me".into()),
                sender_handle: Some("me".into()),
                sender_name: Some("Me".into()),
                sender_avatar: None,
                forwarded_from: Some("Sam Rivera".into()),
                reply_ref: Some(ReplyRefRecord {
                    chat_id: "01SRC".into(),
                    project_id: "people".into(),
                    message_id: "01ORIG".into(),
                    chat_title: "Arbonites".into(),
                    sender: "user:gh_1".into(),
                    sender_name: "Jacob".into(),
                    sender_handle: "unconst".into(),
                    text: "Testing reply in dm".into(),
                }),
            },
        )
        .unwrap();
        doc.commit();
        assert_eq!(doc.message_count(), 1);
        let row: Value = serde_json::from_str(&doc.read_message_json(0).unwrap()).unwrap();
        assert_eq!(row["forwardedFrom"], "Sam Rivera");
        assert_eq!(row["replyRef"]["chatId"], "01SRC");
        assert_eq!(row["replyRef"]["messageId"], "01ORIG");
        assert_eq!(row["replyRef"]["chatTitle"], "Arbonites");
        assert_eq!(row["replyRef"]["text"], "Testing reply in dm");
        assert_eq!(row["role"], "user");
        assert_eq!(row["sender"], "user:me");
        assert_eq!(row["senderHandle"], "me");
        assert_eq!(row["senderName"], "Me");
        assert_eq!(row["replyTo"], "m0");
        assert_eq!(row["mentions"], json!(["user:a", "user:b"]));
        let parts = row["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["kind"], "attachment");
        assert_eq!(parts[0]["attachment"]["name"], "photo.jpg");
        assert_eq!(parts[0]["attachment"]["mime"], "image/jpeg");
        assert_eq!(parts[0]["attachment"]["kind"], "image");
        assert_eq!(parts[0]["attachment"]["ref"], "blob:1");
        assert_eq!(parts[1]["kind"], "text");
        assert_eq!(parts[1]["text"], "hello");
    }

    #[test]
    fn delete_message_drops_row() {
        let doc = ChatDoc::open();
        doc.insert_user_message(
            "keep".into(),
            "stay".into(),
            "dev".into(),
            UserMessageOptsRecord::default(),
        )
        .unwrap();
        doc.insert_user_message(
            "gone".into(),
            "bye".into(),
            "dev".into(),
            UserMessageOptsRecord::default(),
        )
        .unwrap();
        doc.delete_message("gone".into()).unwrap();
        doc.commit();
        assert_eq!(doc.message_count(), 1);
        let row: Value = serde_json::from_str(&doc.read_message_json(0).unwrap()).unwrap();
        assert_eq!(row["id"], "keep");
        doc.delete_message("gone".into()).unwrap();
        doc.commit();
        assert_eq!(doc.message_count(), 1);
    }

    #[test]
    fn edit_message_replaces_text() {
        let doc = ChatDoc::open();
        doc.insert_user_message(
            "m1".into(),
            "hello".into(),
            "dev".into(),
            UserMessageOptsRecord::default(),
        )
        .unwrap();
        doc.edit_message("m1".into(), "hello edited".into())
            .unwrap();
        doc.commit();
        let row: Value = serde_json::from_str(&doc.read_message_json(0).unwrap()).unwrap();
        assert_eq!(row["parts"][0]["text"], "hello edited");
        assert!(row["editedAt"].as_i64().unwrap_or(0) > 0);
    }

    #[test]
    fn toggle_reaction_flat_twice_and_legacy_once_clear() {
        let doc = ChatDoc::open();
        doc.insert_user_message(
            "m-flat".into(),
            "hi".into(),
            "dev".into(),
            UserMessageOptsRecord::default(),
        )
        .unwrap();
        doc.toggle_reaction("m-flat".into(), "👍".into(), "user:a".into())
            .unwrap();
        doc.toggle_reaction("m-flat".into(), "👍".into(), "user:a".into())
            .unwrap();
        doc.commit();
        let flat: Value = serde_json::from_str(&doc.read_message_json(0).unwrap()).unwrap();
        let flat_reacts = flat.get("reactions").cloned().unwrap_or(json!({}));
        assert!(
            flat_reacts
                .get("👍")
                .and_then(|b| b.get("user:a"))
                .is_none(),
            "flat toggle twice should leave no reaction: {flat_reacts}"
        );

        let fixture = std::fs::read(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../arbos-core/fixtures/chat_doc.bin"),
        )
        .expect("chat_doc.bin");
        let legacy_doc = ChatDoc::open_from_snapshot(fixture).unwrap();
        // msg-legacy-react is the last row with 🎉 / user:legacy.
        legacy_doc
            .toggle_reaction("msg-legacy-react".into(), "🎉".into(), "user:legacy".into())
            .unwrap();
        legacy_doc.commit();
        let n = legacy_doc.message_count();
        let mut found = false;
        for i in 0..n {
            let row: Value =
                serde_json::from_str(&legacy_doc.read_message_json(i).unwrap()).unwrap();
            if row["id"] == "msg-legacy-react" {
                let reacts = row.get("reactions").cloned().unwrap_or(json!({}));
                assert!(
                    reacts
                        .get("🎉")
                        .and_then(|b| b.get("user:legacy"))
                        .is_none(),
                    "legacy toggle once should clear: {reacts}"
                );
                found = true;
                break;
            }
        }
        assert!(found, "msg-legacy-react missing from fixture");
    }

    #[test]
    fn add_command_returns_pending_id() {
        let doc = ChatDoc::open();
        let id = doc
            .add_command(
                "send".into(),
                r#"{"messageId":"m1"}"#.into(),
                "dev:1".into(),
            )
            .unwrap();
        assert!(!id.is_empty());
        doc.commit();
        let commands = doc.lock_doc().to_json()["commands"].clone();
        let list = commands.as_array().expect("commands list");
        let row = list
            .iter()
            .find(|c| c["id"] == id)
            .expect("command id in list");
        assert_eq!(row["kind"], "send");
        assert_eq!(row["status"], "pending");
        assert_eq!(row["payload"]["messageId"], "m1");
    }

    struct CommitListener {
        hits: Mutex<Vec<Touched>>,
    }

    impl DocChangeListener for CommitListener {
        fn on_change(&self, touched: Touched) {
            self.hits.lock().unwrap().push(touched);
        }
    }

    #[test]
    fn commit_after_write_notifies_listener_once_list_changed() {
        let doc = Arc::new(ChatDoc::open());
        let listener = Arc::new(CommitListener {
            hits: Mutex::new(Vec::new()),
        });
        doc.on_change(listener.clone());
        doc.insert_user_message(
            "m1".into(),
            "hi".into(),
            "dev".into(),
            UserMessageOptsRecord::default(),
        )
        .unwrap();
        assert!(
            listener.hits.lock().unwrap().is_empty(),
            "writers must not notify before commit"
        );
        doc.commit();
        let hits = listener.hits.lock().unwrap();
        assert_eq!(hits.len(), 1, "commit should notify exactly once");
        assert!(hits[0].list_changed, "insert should set list_changed");
    }

    struct RecordingLocalUpdate {
        updates: Mutex<Vec<Vec<u8>>>,
    }

    impl LocalUpdateListener for RecordingLocalUpdate {
        fn on_local_update(&self, update: Vec<u8>) {
            self.updates.lock().unwrap().push(update);
        }
    }

    #[test]
    fn local_update_after_commit_applies_to_peer() {
        let doc = Arc::new(ChatDoc::open());
        let listener = Arc::new(RecordingLocalUpdate {
            updates: Mutex::new(Vec::new()),
        });
        doc.on_local_update(listener.clone());
        doc.insert_user_message(
            "m1".into(),
            "hi".into(),
            "dev".into(),
            UserMessageOptsRecord::default(),
        )
        .unwrap();
        doc.commit();
        let updates = listener.updates.lock().unwrap().clone();
        assert_eq!(
            updates.len(),
            1,
            "commit should deliver exactly one local update"
        );
        let peer = ChatDoc::open();
        peer.apply(updates[0].clone()).unwrap();
        assert_eq!(doc.read_rows_json(), peer.read_rows_json());
    }

    #[test]
    fn apply_remote_update_skips_local_update_listener() {
        let source = ChatDoc::open();
        source
            .insert_user_message(
                "m1".into(),
                "hi".into(),
                "dev".into(),
                UserMessageOptsRecord::default(),
            )
            .unwrap();
        source.commit();
        let remote = source.export_updates_since(Vec::new());

        let doc = Arc::new(ChatDoc::open());
        let listener = Arc::new(RecordingLocalUpdate {
            updates: Mutex::new(Vec::new()),
        });
        doc.on_local_update(listener.clone());
        doc.apply(remote).unwrap();
        assert!(
            listener.updates.lock().unwrap().is_empty(),
            "imported remote rows must not reach the local-update listener"
        );
    }

    struct ReentrantLocalUpdate {
        doc: Arc<ChatDoc>,
        saw_rows: Mutex<bool>,
    }

    impl LocalUpdateListener for ReentrantLocalUpdate {
        fn on_local_update(&self, _update: Vec<u8>) {
            let _ = self.doc.read_rows_json();
            *self.saw_rows.lock().unwrap() = true;
        }
    }

    #[test]
    fn local_update_callback_does_not_deadlock_on_reentrant_read() {
        let doc = Arc::new(ChatDoc::open());
        let listener = Arc::new(ReentrantLocalUpdate {
            doc: doc.clone(),
            saw_rows: Mutex::new(false),
        });
        doc.on_local_update(listener.clone());
        doc.insert_user_message(
            "m1".into(),
            "hi".into(),
            "dev".into(),
            UserMessageOptsRecord::default(),
        )
        .unwrap();
        doc.commit();
        assert!(
            *listener.saw_rows.lock().unwrap(),
            "listener should have run after commit"
        );
    }

    fn long_transcript(rows: usize) -> ChatDoc {
        let doc = ChatDoc::open();
        for i in 0..rows {
            doc.insert_user_message(
                format!("m{i}"),
                format!("row {i}: a line of chat with a few words in it"),
                "dev".into(),
                UserMessageOptsRecord::default(),
            )
            .unwrap();
        }
        doc.commit();
        doc
    }

    /// Reading a long transcript one row at a time (the iPhone's `readMessage`
    /// path) must cost about what one `read_rows_json` costs — not rows × that.
    /// Before 2026-10-23 `read_message_json` projected every row and picked one:
    /// a 5 000-row group took 86 s per snapshot in release.
    #[test]
    fn per_index_reads_stay_linear_in_transcript_length() {
        let rows = 2_000usize;
        let doc = long_transcript(rows);
        assert_eq!(doc.message_count(), rows as u64);

        let started = std::time::Instant::now();
        let all = doc.read_rows_json();
        let one_shot = started.elapsed();

        let started = std::time::Instant::now();
        let mut per_index = Vec::with_capacity(rows);
        for i in 0..rows as u64 {
            per_index.push(doc.read_message_json(i).expect("row exists"));
        }
        let indexed = started.elapsed();

        let all: Vec<Value> = serde_json::from_str(&all).unwrap();
        for (row, json) in all.iter().zip(&per_index) {
            assert_eq!(row, &serde_json::from_str::<Value>(json).unwrap());
        }
        // Loose bound: the per-index path pays a lock and a list lookup per row
        // (a handful of times the one-shot cost); the quadratic path was ~1000×.
        let budget = one_shot * 25 + std::time::Duration::from_millis(50);
        assert!(
            indexed <= budget,
            "reading {rows} rows by index took {indexed:?}; one-shot read took {one_shot:?} (budget {budget:?})"
        );
    }
}
