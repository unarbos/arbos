//! Sans-IO room sync session (crdt-sync §7–§8.6, §11).
//!
//! A client opens a socket, feeds bytes in, and sends bytes out. Handshake,
//! catch-up, push/ack, resend, and reconnect backoff live here — no tokio or
//! tungstenite. The Mac (and later UniFFI/Swift) drives [`RoomSession`] from
//! its I/O loop and performs the queued [`RoomAction`]s.

use std::collections::VecDeque;
use std::time::Duration;

use arbos_core::ids::ulid;
use arbos_core::wire::{
    self, Ack, ErrorHeader, Frame, FrameType, Hello, PushHeader, RowHeader, RowsReq, StatusHeader,
    WakeHeader, ROW_MAX,
};
use arbos_core::PROTO_VERSION;
use serde::Deserialize;
use serde_json::{json, Value};

/// Lenient `STATE` header for clients (crdt-sync §7). Relay still writes
/// [`wire::State`]; unknown `box` values and missing fields must not stall
/// the handshake — only the box update is skipped.
#[derive(Debug, Deserialize)]
struct StateHeader {
    #[serde(rename = "checkpointSeq", default)]
    checkpoint_seq: i64,
    #[serde(rename = "box", default)]
    box_state: Option<String>,
}

fn parse_box_state(value: &str) -> Option<wire::BoxState> {
    match value {
        "awake" => Some(wire::BoxState::Awake),
        "waking" => Some(wire::BoxState::Waking),
        "asleep" => Some(wire::BoxState::Asleep),
        "error" => Some(wire::BoxState::Error),
        _ => None,
    }
}

/// Wait this long after `on_open` for a `STATE` frame before giving up.
pub const HELLO_DEADLINE: Duration = Duration::from_secs(15);
/// Presence cadence hint for hosts that drive ticks (Mac/iOS ~15 s).
pub const PRESENCE_EVERY: Duration = Duration::from_secs(15);
/// Reconnect backoff floor (§8.6).
pub const BACKOFF_MIN: Duration = Duration::from_millis(250);
/// Reconnect backoff ceiling (§8.6).
pub const BACKOFF_MAX: Duration = Duration::from_secs(16);
/// After a connection that stayed up at least this long, reset backoff to min.
pub const BACKOFF_RESET_AFTER: Duration = Duration::from_secs(30);
/// Default wait before resending after `rate_limited` when the error has no `retryS`.
pub const RATE_LIMIT_RETRY_DEFAULT: Duration = Duration::from_secs(5);
/// Cap on the doubled wait between `rate_limited` refusals.
pub const RATE_LIMIT_RETRY_MAX: Duration = Duration::from_secs(60);
/// First wait before resending after `not_available` on this connection.
pub const PUSH_REFUSE_RETRY_DEFAULT: Duration = Duration::from_secs(60);
/// Cap on the doubled wait between `not_available` retries (10 min).
pub const PUSH_REFUSE_RETRY_MAX: Duration = Duration::from_secs(600);
/// Pending batch ids for history heals start with this prefix so a resend keeps `heal: true`.
pub const HEAL_BATCH_PREFIX: &str = "heal-";

/// Room path for the WebSocket URL: `people/{chat}` or `{project}/{chat}`.
pub fn room_path(project_id: &str, chat_id: &str, is_people: bool) -> String {
    if is_people {
        format!("people/{chat_id}")
    } else {
        wire::room_name(project_id, chat_id)
    }
}

/// What the host should do next. Drain with [`RoomSession::drain_actions`].
#[derive(Debug, Clone)]
pub enum RoomAction {
    /// Write these frame bytes on the socket.
    SendFrame(Vec<u8>),
    /// `GET /room/{room}/checkpoint` then call [`RoomSession::on_checkpoint`]
    /// (or [`RoomSession::on_checkpoint_failed`]).
    FetchCheckpoint {
        room: String,
        frontier: Vec<u8>,
        checkpoint_seq: i64,
    },
    /// Import these Loro bytes into the room doc.
    ApplyBytes(Vec<u8>),
    /// Persist the room cursor (and mark the store dirty).
    Cursor(i64),
    /// Persist the pending PUSH queue.
    Pending(Vec<(String, Vec<u8>)>),
    /// Box lifecycle from `STATE` / `STATUS`.
    /// `poll_machine`: true for `STATUS` (host refreshes `GET /machine`).
    BoxState {
        state: wire::BoxState,
        detail: Option<String>,
        poll_machine: bool,
    },
    /// Catch-up finished (`ROWS_DONE`).
    LinkLive,
    /// `ROWS_DONE` carried the relay's version vector for the room. Host: export the doc's
    /// updates since `relay_vv` and **always** call [`RoomSession::on_heal`] with the result
    /// (empty when nothing is missing) — the relay may have lost rows this device still holds
    /// (#1518); `on_heal` sends any heal then the held pending resend.
    HealCheck { relay_vv: Vec<u8> },
    /// Socket died or handshake failed; host should reconnect after `after`.
    ScheduleReconnect { after: Duration },
    /// Relay error the host should know about. `lagging` is handled inside the session;
    /// `rate_limited` stays Live (no this action); `not_available` refuses pushes with no redial
    /// and also emits [`RoomAction::PushRefused`].
    SessionError { code: String, message: String },
    /// A `not_available` refusal for a push on this connection (`batchId` from the error
    /// header). Host: keep the room link live; show a per-chat notice. Cleared by the next
    /// [`RoomAction::Ack`]. Since [feedback blocked-state-relay-2026-10-26] the frame also says
    /// why — `reason` (`blocked_by` / `blocks_member` / `silenced` / `chat_unavailable`) and,
    /// on a block, the other party (`by_handle`, `by_name`) — so the notice can name them; the
    /// code itself never changes, so older clients read it as before.
    PushRefused {
        batch_id: Option<String>,
        code: String,
        reason: Option<String>,
        by_handle: Option<String>,
        by_name: Option<String>,
    },
    /// Another device's presence / call / typing (host maps to UI events).
    /// `header` is the full inbound PRESENCE header (Swift reads `user` / `at`).
    Presence {
        device: Option<String>,
        handle: Option<String>,
        typing: Option<bool>,
        in_call: Option<String>,
        mine: bool,
        header: Value,
    },
    /// Local update exceeded [`ROW_MAX`]; dropped (host may log).
    DroppedOversize { bytes: usize },
    /// A pending PUSH was acknowledged (host may clear UI “unacked” marks).
    Ack { batch_id: String, seq: i64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Before `on_open`.
    Idle,
    /// HELLO sent; waiting for STATE (or HELLO_DEADLINE).
    WaitState,
    /// STATE seen; waiting for host checkpoint resolution.
    WaitCheckpoint,
    /// Catch-up / live.
    Live,
    /// Closed; host should reconnect.
    Closed,
}

/// Sans-IO state machine for one room connection attempt (and reconnect policy).
pub struct RoomSession {
    device_id: String,
    room: String,
    cursor: i64,
    /// Cursor before the most recent [`Self::on_row`] advance. Restored by
    /// [`Self::apply_failed`] so reconnect catch-up re-requests the failed row.
    cursor_before_apply: i64,
    pending: Vec<(String, Vec<u8>)>,
    phase: Phase,
    actions: VecDeque<RoomAction>,
    backoff: Duration,
    /// Host clock at `on_open` (for HELLO deadline and healthy-session reset).
    opened_at_ms: Option<u64>,
    /// Host clock when the last healthy session began (successful live).
    live_since_ms: Option<u64>,
    /// Pending STATE checkpoint fields while host resolves containment/fetch.
    checkpoint_seq: i64,
    /// Delay chosen by the last [`Self::on_close`] / error close.
    reconnect_after: Option<Duration>,
    /// Last host clock seen (`on_open` / `on_tick` / `on_close`).
    last_now_ms: u64,
    /// Latest wake reason while not Live (kept across close / prepare_reconnect).
    held_wake: Option<String>,
    /// Latest outbound presence header while not Live (same retention).
    held_presence: Option<Value>,
    /// A history heal was pushed (or answered empty) on this connection (one per connection;
    /// reset by `prepare_reconnect`).
    healed: bool,
    /// `ROWS_DONE` asked for a heal; pending PUSH resend waits for [`Self::on_heal`].
    awaiting_heal: bool,
    /// Host clock when to resend pending after `rate_limited` (one timer per room).
    rate_limit_resend_at_ms: Option<u64>,
    /// Wait used for the next `rate_limited` schedule; 0 → take `retryS` (or 5 s) from the error.
    /// Doubles up to [`RATE_LIMIT_RETRY_MAX`] after each timed resend; reset on ACK.
    rate_limit_backoff_ms: u64,
    /// `not_available` on this connection: queue local commits, do not PUSH until cleared.
    /// Cleared by any ACK or by [`Self::prepare_reconnect`] (refusal is per connection).
    push_refused: bool,
    /// Host clock when to resend pending after `not_available` (one timer per connection).
    push_refuse_resend_at_ms: Option<u64>,
    /// Wait used for the next `not_available` schedule; 0 → [`PUSH_REFUSE_RETRY_DEFAULT`].
    /// Doubles up to [`PUSH_REFUSE_RETRY_MAX`] after each timed resend; reset on ACK.
    push_refuse_backoff_ms: u64,
    /// Lowest row seq whose import Loro parked as *pending* (its dependencies are not in the
    /// doc yet). Parked ops live only in memory: a relaunch loses them, so the persisted cursor
    /// never passes this row and the next catch-up re-requests it (Loro drops what it already
    /// has). Cleared when a catch-up finishes with no pending import or a checkpoint covers it.
    first_pending_seq: Option<i64>,
    /// An import was parked during this connection.
    pending_this_session: bool,
}

impl RoomSession {
    pub fn new(
        device_id: impl Into<String>,
        room: impl Into<String>,
        cursor: i64,
        pending: Vec<(String, Vec<u8>)>,
    ) -> Self {
        Self {
            device_id: device_id.into(),
            room: room.into(),
            cursor,
            cursor_before_apply: cursor,
            pending,
            phase: Phase::Idle,
            actions: VecDeque::new(),
            backoff: BACKOFF_MIN,
            opened_at_ms: None,
            live_since_ms: None,
            checkpoint_seq: 0,
            reconnect_after: None,
            last_now_ms: 0,
            held_wake: None,
            held_presence: None,
            healed: false,
            awaiting_heal: false,
            rate_limit_resend_at_ms: None,
            rate_limit_backoff_ms: 0,
            push_refused: false,
            push_refuse_resend_at_ms: None,
            push_refuse_backoff_ms: 0,
            first_pending_seq: None,
            pending_this_session: false,
        }
    }

    pub fn room(&self) -> &str {
        &self.room
    }

    /// The cursor to persist and to resume from: the last row walked, held back to just below
    /// the first row whose import is still pending (see `first_pending_seq`).
    pub fn cursor(&self) -> i64 {
        match self.first_pending_seq {
            Some(p) => self.cursor.min(p - 1),
            None => self.cursor,
        }
    }

    /// Lowest row seq still parked by Loro, if any (diagnostics / health).
    pub fn first_pending_seq(&self) -> Option<i64> {
        self.first_pending_seq
    }

    pub fn pending(&self) -> &[(String, Vec<u8>)] {
        &self.pending
    }

    pub fn backoff(&self) -> Duration {
        self.backoff
    }

    /// Sleep this long before the next dial (set by the last close).
    pub fn reconnect_after(&self) -> Duration {
        self.reconnect_after.unwrap_or(self.backoff)
    }

    pub fn is_live(&self) -> bool {
        self.phase == Phase::Live
    }

    pub fn is_closed(&self) -> bool {
        self.phase == Phase::Closed
    }

    pub fn drain_actions(&mut self) -> Vec<RoomAction> {
        self.actions.drain(..).collect()
    }

    fn push(&mut self, action: RoomAction) {
        self.actions.push_back(action);
    }

    fn encode(ty: FrameType, header: Value, payload: Vec<u8>) -> Vec<u8> {
        Frame::new(ty, header, payload).encode().to_vec()
    }

    /// Socket is up. Sends `HELLO` and starts the STATE deadline.
    pub fn on_open(&mut self, now_ms: u64) {
        self.phase = Phase::WaitState;
        self.opened_at_ms = Some(now_ms);
        self.last_now_ms = now_ms;
        self.live_since_ms = None;
        self.pending_this_session = false;
        let hello = Hello {
            cursor: self.cursor(),
            device: self.device_id.clone(),
            room: self.room.clone(),
            proto: PROTO_VERSION,
        };
        let header = serde_json::to_value(&hello).expect("hello");
        self.push(RoomAction::SendFrame(Self::encode(
            FrameType::Hello,
            header,
            Vec::new(),
        )));
    }

    /// Inbound WebSocket binary frame.
    pub fn on_frame(&mut self, bytes: &[u8]) {
        if self.phase == Phase::Closed || self.phase == Phase::Idle {
            return;
        }
        let Ok(Some(frame)) = Frame::decode(bytes.to_vec().into()) else {
            return;
        };
        match frame.ty {
            FrameType::State => self.on_state(frame),
            FrameType::Row => self.on_row(frame),
            FrameType::RowsDone => self.on_rows_done(frame),
            FrameType::Ack => self.on_ack(frame),
            FrameType::Status => self.on_status(frame),
            FrameType::Error => self.on_error(frame),
            FrameType::Presence => self.on_presence(frame),
            FrameType::ProbeOk
            | FrameType::Hello
            | FrameType::RowsReq
            | FrameType::Push
            | FrameType::Probe
            | FrameType::Wake => {}
        }
    }

    fn on_state(&mut self, frame: Frame) {
        if self.phase != Phase::WaitState {
            return;
        }
        let Ok(state) = serde_json::from_value::<StateHeader>(frame.header) else {
            return;
        };
        if let Some(box_value) = state.box_state.as_deref() {
            if let Some(box_state) = parse_box_state(box_value) {
                self.push(RoomAction::BoxState {
                    state: box_state,
                    detail: None,
                    poll_machine: false,
                });
            }
        }
        let frontier = frame.payload.to_vec();
        self.checkpoint_seq = state.checkpoint_seq;
        if state.checkpoint_seq > 0 {
            self.phase = Phase::WaitCheckpoint;
            self.push(RoomAction::FetchCheckpoint {
                room: self.room.clone(),
                frontier,
                checkpoint_seq: state.checkpoint_seq,
            });
        } else {
            self.finish_handshake();
        }
    }

    /// Host: local oplog already includes the STATE frontier — advance cursor
    /// and continue (crdt-sync §7.3).
    pub fn on_checkpoint_skipped(&mut self) {
        if self.phase != Phase::WaitCheckpoint {
            return;
        }
        self.cursor = self.cursor.max(self.checkpoint_seq);
        self.cursor_before_apply = self.cursor;
        self.release_pending_below(self.checkpoint_seq);
        self.push(RoomAction::Cursor(self.cursor()));
        self.finish_handshake();
    }

    /// Host imported checkpoint bytes (or applied them itself). Advances the
    /// cursor and continues catch-up. Does not emit [`RoomAction::ApplyBytes`]
    /// — the host already has the snapshot.
    pub fn on_checkpoint(&mut self, seq: i64) {
        if self.phase != Phase::WaitCheckpoint {
            return;
        }
        self.cursor = self.cursor.max(seq.max(self.checkpoint_seq));
        self.cursor_before_apply = self.cursor;
        // The checkpoint is a whole doc up to its seq: whatever was parked below it is in it.
        self.release_pending_below(seq.max(self.checkpoint_seq));
        self.push(RoomAction::Cursor(self.cursor()));
        self.finish_handshake();
    }

    fn release_pending_below(&mut self, seq: i64) {
        if self.first_pending_seq.is_some_and(|p| p <= seq) {
            self.first_pending_seq = None;
        }
    }

    /// Host: the import of the [`RoomAction::ApplyBytes`] just handled was parked by Loro as
    /// pending (its dependencies are missing). The row counts as walked but not as applied: the
    /// persisted cursor stays below it (a corrective [`RoomAction::Cursor`] follows) and the
    /// next catch-up asks for it again. Nothing reconnects — the deps may arrive live.
    pub fn apply_pending(&mut self) {
        if matches!(self.phase, Phase::Idle | Phase::Closed) {
            return;
        }
        let seq = self.cursor;
        self.first_pending_seq = Some(match self.first_pending_seq {
            Some(p) => p.min(seq),
            None => seq,
        });
        self.pending_this_session = true;
        self.push(RoomAction::Cursor(self.cursor()));
    }

    /// Host could not fetch/import the checkpoint; continue with RowsReq anyway
    /// (same as today's Mac: log and proceed).
    pub fn on_checkpoint_failed(&mut self) {
        if self.phase != Phase::WaitCheckpoint {
            return;
        }
        self.finish_handshake();
    }

    fn finish_handshake(&mut self) {
        self.phase = Phase::Live;
        // Own rows are requested too: a row pushed by this device but saved
        // before its ACK is only on the relay. Loro drops duplicates.
        let req = RowsReq {
            after: self.cursor(),
            exclude_own: false,
        };
        let header = serde_json::to_value(&req).expect("rows_req");
        self.push(RoomAction::SendFrame(Self::encode(
            FrameType::RowsReq,
            header,
            Vec::new(),
        )));
        // Pending PUSH resend waits for ROWS_DONE: when the relay sends a version
        // vector, heal first so covered batches ACK as dup before ordinary resends.
        // Flush wake / presence held while reconnecting or mid-handshake.
        if let Some(reason) = self.held_wake.take() {
            let header = serde_json::to_value(WakeHeader { reason }).expect("wake");
            self.push(RoomAction::SendFrame(Self::encode(
                FrameType::Wake,
                header,
                Vec::new(),
            )));
        }
        if let Some(header) = self.held_presence.take() {
            self.push(RoomAction::SendFrame(Self::encode(
                FrameType::Presence,
                header,
                Vec::new(),
            )));
        }
    }

    /// Resend every unACKed pending batch. Heal batches keep `heal: true` via the
    /// [`HEAL_BATCH_PREFIX`] on their id (no store format change).
    ///
    /// Callers that must not send while this connection has a live `not_available`
    /// refusal (fresh local commits) skip the send themselves; the timed retry
    /// and post-handshake heal path call this even while refused.
    fn resend_pending(&mut self) {
        if self.phase != Phase::Live || self.awaiting_heal {
            return;
        }
        let pending = self.pending.clone();
        for (batch_id, bytes) in pending {
            let header = serde_json::to_value(PushHeader {
                heal: batch_id.starts_with(HEAL_BATCH_PREFIX),
                batch_id,
            })
            .expect("push");
            self.push(RoomAction::SendFrame(Self::encode(
                FrameType::Push,
                header,
                bytes,
            )));
        }
    }

    fn on_row(&mut self, frame: Frame) {
        if self.phase != Phase::Live {
            return;
        }
        let Ok(header) = serde_json::from_value::<RowHeader>(frame.header) else {
            return;
        };
        self.push(RoomAction::ApplyBytes(frame.payload.to_vec()));
        // Tentative: host skips the following Cursor and calls [`Self::apply_failed`]
        // if import fails, restoring `cursor_before_apply`.
        self.cursor_before_apply = self.cursor;
        self.cursor = self.cursor.max(header.seq);
        self.push(RoomAction::Cursor(self.cursor()));
    }

    fn on_rows_done(&mut self, frame: Frame) {
        if self.phase != Phase::Live {
            return;
        }
        let head_seq = frame
            .header
            .get("headSeq")
            .and_then(Value::as_i64)
            .unwrap_or(self.cursor);
        self.cursor = self.cursor.max(head_seq);
        self.cursor_before_apply = self.cursor;
        // A catch-up that re-walked the parked range and parked nothing has applied it all.
        if !self.pending_this_session {
            self.first_pending_seq = None;
        }
        self.push(RoomAction::Cursor(self.cursor()));
        self.push(RoomAction::LinkLive);
        if !frame.payload.is_empty() && !self.healed {
            // Hold pending until the host answers; heal (if any) goes out first.
            self.awaiting_heal = true;
            self.push(RoomAction::HealCheck {
                relay_vv: frame.payload.to_vec(),
            });
        } else {
            // Older relay (no version vector), or already healed this connection.
            self.resend_pending();
        }
    }

    /// Host answer to [`RoomAction::HealCheck`]: `bytes` are the doc's updates the relay's
    /// version vector lacked (empty when nothing is missing). Hosts must call this even for
    /// an empty result so the held pending resend can proceed. A non-empty answer is pushed
    /// once per connection as a `heal-…` batch; older `heal-` pending batches are dropped
    /// (the new heal carries everything beyond the relay's vector).
    pub fn on_heal(&mut self, bytes: Vec<u8>) {
        if self.phase != Phase::Live {
            return;
        }
        if self.healed && !self.awaiting_heal {
            return;
        }
        let was_awaiting = self.awaiting_heal;
        self.awaiting_heal = false;
        self.healed = true;

        let queued_heal = if bytes.is_empty() {
            false
        } else if bytes.len() > ROW_MAX {
            self.push(RoomAction::DroppedOversize { bytes: bytes.len() });
            if was_awaiting {
                self.resend_pending();
            }
            return;
        } else {
            self.pending
                .retain(|(id, _)| !id.starts_with(HEAL_BATCH_PREFIX));
            let batch_id = format!("{HEAL_BATCH_PREFIX}{}", ulid());
            // Heal first so the relay can ACK covered pending batches as dup.
            self.pending.insert(0, (batch_id, bytes));
            self.push(RoomAction::Pending(self.pending.clone()));
            true
        };

        if was_awaiting {
            self.resend_pending();
        } else if queued_heal {
            // Direct heal (no held resend): send only the heal just queued.
            if let Some((batch_id, payload)) = self.pending.first().cloned() {
                let header = serde_json::to_value(PushHeader {
                    heal: true,
                    batch_id,
                })
                .expect("push");
                self.push(RoomAction::SendFrame(Self::encode(
                    FrameType::Push,
                    header,
                    payload,
                )));
            }
        }
    }

    fn on_ack(&mut self, frame: Frame) {
        // Apply during WaitState / WaitCheckpoint too: a local PUSH can race the
        // handshake (ACK can arrive while waiting for STATE). Ignoring the
        // ACK until Live leaves the batch in `pending` forever when the only ACK
        // arrived before STATE and a later resend is treated as dup.
        //
        // Do not move the cursor on ACK: the relay can ACK while rows from other
        // connections are still queued for this socket. Advancing past those rows
        // would skip them on the next RowsReq (crdt-sync: cursor only moves on
        // ROW / ROWS_DONE / checkpoint). Own rows still arrive as ROW broadcasts.
        if matches!(self.phase, Phase::Idle | Phase::Closed) {
            return;
        }
        let Ok(header) = serde_json::from_value::<Ack>(frame.header) else {
            return;
        };
        let before = self.pending.len();
        self.pending.retain(|(id, _)| *id != header.batch_id);
        if self.pending.len() != before {
            self.push(RoomAction::Pending(self.pending.clone()));
            // A successful ACK ends the rate-limit backoff chain.
            self.rate_limit_backoff_ms = 0;
            self.rate_limit_resend_at_ms = None;
        }
        // Any ACK in the room clears a `not_available` refusal so the next push goes out.
        self.push_refused = false;
        self.push_refuse_backoff_ms = 0;
        self.push_refuse_resend_at_ms = None;
        self.push(RoomAction::Ack {
            batch_id: header.batch_id,
            seq: header.seq,
        });
    }

    fn on_status(&mut self, frame: Frame) {
        let Ok(header) = serde_json::from_value::<StatusHeader>(frame.header) else {
            return;
        };
        self.push(RoomAction::BoxState {
            state: header.box_state,
            detail: header.detail,
            poll_machine: true,
        });
    }

    fn on_error(&mut self, frame: Frame) {
        let Ok(header) = serde_json::from_value::<ErrorHeader>(frame.header) else {
            return;
        };
        if header.code == "lagging" {
            // Live: ask again with the same excludeOwn as finish_handshake
            // (own rows may exist only on the relay). Mid-handshake
            // (WaitState / WaitCheckpoint): ignore — finish_handshake resyncs.
            if self.phase == Phase::Live {
                let req = RowsReq {
                    after: self.cursor(),
                    exclude_own: false,
                };
                let h = serde_json::to_value(&req).expect("rows_req");
                self.push(RoomAction::SendFrame(Self::encode(
                    FrameType::RowsReq,
                    h,
                    Vec::new(),
                )));
            }
            return;
        }
        if header.code == "rate_limited" {
            // Stay Live; keep pending; one resend timer per room.
            if self.rate_limit_resend_at_ms.is_none() && self.phase == Phase::Live {
                let wait_ms = if self.rate_limit_backoff_ms > 0 {
                    self.rate_limit_backoff_ms
                } else {
                    let retry_s = header
                        .retry_s
                        .filter(|s| *s > 0)
                        .unwrap_or(RATE_LIMIT_RETRY_DEFAULT.as_secs() as i64)
                        as u64;
                    retry_s.saturating_mul(1000)
                };
                self.rate_limit_backoff_ms = wait_ms;
                self.rate_limit_resend_at_ms = Some(self.last_now_ms.saturating_add(wait_ms));
            }
            return;
        }
        if header.code == "not_available" {
            // Stay Live; keep pending; no redial. Refusal is for this connection only.
            self.push_refused = true;
            self.rate_limit_resend_at_ms = None;
            if self.push_refuse_resend_at_ms.is_none() && self.phase == Phase::Live {
                let wait_ms = if self.push_refuse_backoff_ms > 0 {
                    self.push_refuse_backoff_ms
                } else {
                    PUSH_REFUSE_RETRY_DEFAULT.as_millis() as u64
                };
                self.push_refuse_backoff_ms = wait_ms;
                self.push_refuse_resend_at_ms = Some(self.last_now_ms.saturating_add(wait_ms));
            }
            self.push(RoomAction::PushRefused {
                batch_id: header.batch_id,
                code: header.code.clone(),
                reason: header.reason,
                by_handle: header.by_handle,
                by_name: header.by_name,
            });
            self.push(RoomAction::SessionError {
                code: header.code,
                message: header.message,
            });
            return;
        }
        self.push(RoomAction::SessionError {
            code: header.code,
            message: header.message,
        });
        self.close_for_reconnect(Some(self.last_now_ms));
    }

    fn on_presence(&mut self, frame: Frame) {
        let h = &frame.header;
        let device = h.get("device").and_then(Value::as_str).map(str::to_string);
        let mine = device.as_deref() == Some(self.device_id.as_str());
        self.push(RoomAction::Presence {
            device,
            handle: h
                .get("handle")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            typing: h.get("typing").and_then(Value::as_bool),
            in_call: h.get("in_call").and_then(Value::as_str).map(str::to_string),
            mine,
            header: frame.header,
        });
    }

    /// Local Loro commit bytes → `PUSH` (or queue if not live yet — host should
    /// only call this while a session is open; pending is resent after catch-up).
    ///
    /// Always appends to pending and emits [`RoomAction::Pending`]. While this
    /// connection has a live `not_available` refusal, the frame is not sent.
    pub fn on_local_commit(&mut self, bytes: Vec<u8>) {
        if bytes.len() > ROW_MAX {
            self.push(RoomAction::DroppedOversize { bytes: bytes.len() });
            return;
        }
        let batch_id = ulid();
        self.pending.push((batch_id.clone(), bytes.clone()));
        self.push(RoomAction::Pending(self.pending.clone()));
        if self.phase == Phase::Live && !self.awaiting_heal && !self.push_refused {
            let header = serde_json::to_value(PushHeader {
                batch_id,
                heal: false,
            })
            .expect("push");
            self.push(RoomAction::SendFrame(Self::encode(
                FrameType::Push,
                header,
                bytes,
            )));
        }
    }

    /// Host clock tick: enforce HELLO deadline; mark live-since on first live tick;
    /// fire the single `rate_limited` / `not_available` pending resend when due.
    pub fn on_tick(&mut self, now_ms: u64) {
        self.last_now_ms = now_ms;
        if self.phase == Phase::WaitState {
            if let Some(opened) = self.opened_at_ms {
                if now_ms.saturating_sub(opened) >= HELLO_DEADLINE.as_millis() as u64 {
                    self.push(RoomAction::SessionError {
                        code: "state_timeout".into(),
                        message: "STATE did not arrive".into(),
                    });
                    self.close_for_reconnect(Some(now_ms));
                }
            }
        }
        if self.phase == Phase::Live && self.live_since_ms.is_none() {
            self.live_since_ms = Some(now_ms);
        }
        if self.phase == Phase::Live {
            if let Some(at) = self.rate_limit_resend_at_ms {
                if now_ms >= at {
                    let used = self
                        .rate_limit_backoff_ms
                        .max(RATE_LIMIT_RETRY_DEFAULT.as_millis() as u64);
                    self.rate_limit_resend_at_ms = None;
                    self.rate_limit_backoff_ms = used
                        .saturating_mul(2)
                        .min(RATE_LIMIT_RETRY_MAX.as_millis() as u64);
                    self.resend_pending();
                }
            }
            if let Some(at) = self.push_refuse_resend_at_ms {
                if now_ms >= at {
                    let used = self
                        .push_refuse_backoff_ms
                        .max(PUSH_REFUSE_RETRY_DEFAULT.as_millis() as u64);
                    self.push_refuse_resend_at_ms = None;
                    self.push_refuse_backoff_ms = used
                        .saturating_mul(2)
                        .min(PUSH_REFUSE_RETRY_MAX.as_millis() as u64);
                    self.resend_pending();
                }
            }
        }
    }

    /// Socket closed or host is abandoning this attempt.
    pub fn on_close(&mut self, now_ms: u64) {
        self.last_now_ms = now_ms;
        self.close_for_reconnect(Some(now_ms));
    }

    /// Host failed to import the [`RoomAction::ApplyBytes`] that preceded a
    /// [`RoomAction::Cursor`] in the same drain. Restores the cursor to the last
    /// successfully imported row and schedules an immediate reconnect so catch-up
    /// re-requests from there. Does not emit a Cursor action (host must skip the
    /// one already queued).
    pub fn apply_failed(&mut self) {
        if matches!(self.phase, Phase::Idle | Phase::Closed) {
            return;
        }
        self.cursor = self.cursor_before_apply;
        // Immediate reconnect; do not bump backoff — this is not a transport fault.
        self.reconnect_after = Some(Duration::ZERO);
        self.phase = Phase::Closed;
        self.opened_at_ms = None;
        self.live_since_ms = None;
        self.push(RoomAction::ScheduleReconnect {
            after: Duration::ZERO,
        });
    }

    fn close_for_reconnect(&mut self, now_ms: Option<u64>) {
        if self.phase == Phase::Closed {
            return;
        }
        // Reset backoff after a healthy open longer than BACKOFF_RESET_AFTER.
        let healthy = match (self.opened_at_ms, now_ms) {
            (Some(since), Some(now)) => {
                now.saturating_sub(since) > BACKOFF_RESET_AFTER.as_millis() as u64
            }
            _ => false,
        };
        if healthy {
            self.backoff = BACKOFF_MIN;
        }
        let after = self.backoff;
        self.reconnect_after = Some(after);
        self.backoff = (self.backoff.saturating_mul(2)).min(BACKOFF_MAX);
        self.phase = Phase::Closed;
        self.opened_at_ms = None;
        self.live_since_ms = None;
        self.push(RoomAction::ScheduleReconnect { after });
    }

    /// Reset for the next dial after sleeping out [`RoomAction::ScheduleReconnect`].
    pub fn prepare_reconnect(&mut self) {
        self.phase = Phase::Idle;
        self.actions.clear();
        self.checkpoint_seq = 0;
        self.reconnect_after = None;
        self.healed = false;
        self.awaiting_heal = false;
        self.rate_limit_resend_at_ms = None;
        // `rate_limit_backoff_ms` stays: the doubling chain outlives one socket.
        // `push_refused` is per connection: clear so the next dial's heal / pending
        // resend run as normal; another `not_available` re-applies the same rule.
        self.push_refused = false;
        self.push_refuse_resend_at_ms = None;
        self.push_refuse_backoff_ms = 0;
    }

    pub fn wake(&mut self, reason: impl Into<String>) {
        let reason = reason.into();
        if self.phase != Phase::Live {
            // Only the latest wake: the relay uses the reason only to ensure
            // the box is awake; collapsing several wakes loses nothing.
            self.held_wake = Some(reason);
            return;
        }
        let header = serde_json::to_value(WakeHeader { reason }).expect("wake");
        self.push(RoomAction::SendFrame(Self::encode(
            FrameType::Wake,
            header,
            Vec::new(),
        )));
    }

    pub fn presence(&mut self, header: Value) {
        if self.phase != Phase::Live {
            self.held_presence = Some(header);
            return;
        }
        self.push(RoomAction::SendFrame(Self::encode(
            FrameType::Presence,
            header,
            Vec::new(),
        )));
    }

    pub fn typing(&mut self, mut presence_header: Value, active: bool) {
        if let Some(obj) = presence_header.as_object_mut() {
            obj.insert("typing".into(), json!(active));
        }
        self.presence(presence_header);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbos_core::wire::{BoxState, FrameType};

    fn state_frame(checkpoint_seq: i64, frontier: Vec<u8>) -> Vec<u8> {
        let header = json!({
            "headSeq": 10,
            "seqFloor": 0,
            "checkpointSeq": checkpoint_seq,
            "checkpointSize": frontier.len() as i64,
            "rowCount": 0,
            "rowBytes": 0,
            "box": "awake",
        });
        Frame::new(FrameType::State, header, frontier)
            .encode()
            .to_vec()
    }

    fn row_frame(seq: i64, payload: Vec<u8>) -> Vec<u8> {
        let header = json!({
            "seq": seq,
            "device": "dev:other",
            "batchId": "batch-row",
        });
        Frame::new(FrameType::Row, header, payload)
            .encode()
            .to_vec()
    }

    fn rows_done(head: i64) -> Vec<u8> {
        Frame::new(FrameType::RowsDone, json!({ "headSeq": head }), Vec::new())
            .encode()
            .to_vec()
    }

    fn ack_frame(batch_id: &str, seq: i64) -> Vec<u8> {
        Frame::new(
            FrameType::Ack,
            json!({ "batchId": batch_id, "seq": seq }),
            Vec::new(),
        )
        .encode()
        .to_vec()
    }

    fn take_sends(actions: &[RoomAction]) -> Vec<Frame> {
        actions
            .iter()
            .filter_map(|a| match a {
                RoomAction::SendFrame(b) => Frame::decode(b.clone().into()).ok().flatten(),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn room_path_people_and_project() {
        assert_eq!(room_path("p1", "c1", false), "p1/c1");
        assert_eq!(room_path("p1", "c1", true), "people/c1");
    }

    #[test]
    fn connect_catch_up_and_push_resend_before_ack() {
        let mut s = RoomSession::new("dev:me", "proj/chat", 0, Vec::new());
        s.on_open(1_000);
        let open = s.drain_actions();
        let sends = take_sends(&open);
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0].ty, FrameType::Hello);

        // STATE with no checkpoint → RowsReq immediately.
        s.on_frame(&state_frame(0, Vec::new()));
        let after_state = s.drain_actions();
        assert!(after_state.iter().any(|a| matches!(
            a,
            RoomAction::BoxState {
                state: BoxState::Awake,
                poll_machine: false,
                ..
            }
        )));
        let sends = take_sends(&after_state);
        assert!(sends.iter().any(|f| f.ty == FrameType::RowsReq));
        assert!(s.is_live());

        // Catch-up row + done.
        s.on_frame(&row_frame(1, b"row-bytes".to_vec()));
        let row_actions = s.drain_actions();
        assert!(row_actions
            .iter()
            .any(|a| matches!(a, RoomAction::ApplyBytes(b) if b == b"row-bytes")));
        assert!(row_actions
            .iter()
            .any(|a| matches!(a, RoomAction::Cursor(1))));

        s.on_frame(&rows_done(1));
        assert!(s
            .drain_actions()
            .iter()
            .any(|a| matches!(a, RoomAction::LinkLive)));

        // Local commit → PUSH; disconnect before ACK → pending resent on next open.
        s.on_local_commit(b"local-update".to_vec());
        let push_actions = s.drain_actions();
        let pending = s.pending().to_vec();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].1, b"local-update");
        let pushes: Vec<_> = take_sends(&push_actions)
            .into_iter()
            .filter(|f| f.ty == FrameType::Push)
            .collect();
        assert_eq!(pushes.len(), 1);
        assert_eq!(pushes[0].payload.as_ref(), b"local-update");

        s.on_close(2_000);
        let close = s.drain_actions();
        assert!(close.iter().any(|a| matches!(
            a,
            RoomAction::ScheduleReconnect { after } if *after == BACKOFF_MIN
        )));

        // New connection: HELLO, catch-up; pending PUSH waits for ROWS_DONE (no VV).
        s.prepare_reconnect();
        s.on_open(5_000);
        let _ = s.drain_actions();
        s.on_frame(&state_frame(0, Vec::new()));
        let after_state = s.drain_actions();
        assert!(
            take_sends(&after_state)
                .iter()
                .all(|f| f.ty != FrameType::Push),
            "pending held until ROWS_DONE"
        );
        s.on_frame(&rows_done(1));
        let resend = s.drain_actions();
        let pushes: Vec<_> = take_sends(&resend)
            .into_iter()
            .filter(|f| f.ty == FrameType::Push)
            .collect();
        assert_eq!(pushes.len(), 1);
        assert_eq!(pushes[0].payload.as_ref(), b"local-update");
        let batch_id = pending[0].0.clone();
        assert_eq!(
            pushes[0].header.get("batchId").and_then(Value::as_str),
            Some(batch_id.as_str())
        );

        // ACK clears pending.
        s.on_frame(&ack_frame(&batch_id, 2));
        let ack_actions = s.drain_actions();
        assert!(s.pending().is_empty());
        assert!(ack_actions
            .iter()
            .any(|a| matches!(a, RoomAction::Pending(p) if p.is_empty())));
        assert!(ack_actions
            .iter()
            .any(|a| matches!(a, RoomAction::Ack { seq: 2, .. })));
    }

    #[test]
    fn ack_during_wait_state_clears_pending() {
        let pending = vec![("batch-early".into(), b"pushed-before-state".to_vec())];
        let mut s = RoomSession::new("dev:me", "proj/chat", 0, pending);
        s.on_open(1_000);
        let _ = s.drain_actions();
        // Still WaitState: ACK must clear pending (hello-wait parity).
        s.on_frame(&ack_frame("batch-early", 1));
        let actions = s.drain_actions();
        assert!(s.pending().is_empty());
        assert!(actions
            .iter()
            .any(|a| matches!(a, RoomAction::Pending(p) if p.is_empty())));
        assert!(actions
            .iter()
            .any(|a| matches!(a, RoomAction::Ack { seq: 1, .. })));
        // Handshake still completes after STATE.
        s.on_frame(&state_frame(0, Vec::new()));
        let after = s.drain_actions();
        assert!(s.is_live());
        assert!(take_sends(&after)
            .iter()
            .any(|f| f.ty == FrameType::RowsReq));
    }

    #[test]
    fn checkpoint_skipped_when_host_says_contained() {
        let mut s = RoomSession::new("dev:me", "proj/chat", 3, Vec::new());
        s.on_open(1);
        let _ = s.drain_actions();
        s.on_frame(&state_frame(5, b"frontier".to_vec()));
        let actions = s.drain_actions();
        assert!(actions.iter().any(|a| matches!(
            a,
            RoomAction::FetchCheckpoint {
                checkpoint_seq: 5,
                ..
            }
        )));
        s.on_checkpoint_skipped();
        assert_eq!(s.cursor(), 5);
        let cont = s.drain_actions();
        assert!(take_sends(&cont).iter().any(|f| f.ty == FrameType::RowsReq));
    }

    #[test]
    fn state_with_unknown_box_still_finishes_handshake() {
        let mut s = RoomSession::new("dev:me", "proj/chat", 0, Vec::new());
        s.on_open(1_000);
        let _ = s.drain_actions();
        let frame = Frame::new(
            FrameType::State,
            json!({
                "headSeq": 0,
                "seqFloor": 0,
                "checkpointSeq": 0,
                "box": "hibernating",
            }),
            Vec::new(),
        )
        .encode()
        .to_vec();
        s.on_frame(&frame);
        let actions = s.drain_actions();
        assert!(
            !actions
                .iter()
                .any(|a| matches!(a, RoomAction::BoxState { .. })),
            "unknown box skips only the box update"
        );
        assert!(
            !actions
                .iter()
                .any(|a| matches!(a, RoomAction::SessionError { .. })),
            "unknown box must not error the session"
        );
        assert!(take_sends(&actions)
            .iter()
            .any(|f| f.ty == FrameType::RowsReq));
        assert!(s.is_live());
    }

    #[test]
    fn state_with_only_checkpoint_seq_fetches_checkpoint() {
        let mut s = RoomSession::new("dev:me", "proj/chat", 0, Vec::new());
        s.on_open(1_000);
        let _ = s.drain_actions();
        let frame = Frame::new(
            FrameType::State,
            json!({ "checkpointSeq": 5 }),
            b"frontier".to_vec(),
        )
        .encode()
        .to_vec();
        s.on_frame(&frame);
        let actions = s.drain_actions();
        assert!(
            !actions
                .iter()
                .any(|a| matches!(a, RoomAction::BoxState { .. })),
            "missing box emits no BoxState"
        );
        let fetches: Vec<_> = actions
            .iter()
            .filter(|a| matches!(a, RoomAction::FetchCheckpoint { .. }))
            .collect();
        assert_eq!(fetches.len(), 1);
        assert!(matches!(
            &fetches[0],
            RoomAction::FetchCheckpoint {
                checkpoint_seq: 5,
                ..
            }
        ));
    }

    fn lagging_frame() -> Vec<u8> {
        Frame::new(
            FrameType::Error,
            json!({ "code": "lagging", "message": "behind" }),
            Vec::new(),
        )
        .encode()
        .to_vec()
    }

    #[test]
    fn lagging_while_live_rows_req_include_own() {
        let mut s = RoomSession::new("dev:me", "proj/chat", 4, Vec::new());
        s.on_open(1_000);
        let _ = s.drain_actions();
        s.on_frame(&state_frame(0, Vec::new()));
        let _ = s.drain_actions();
        assert!(s.is_live());

        s.on_frame(&lagging_frame());
        let actions = s.drain_actions();
        assert!(
            !actions
                .iter()
                .any(|a| matches!(a, RoomAction::ScheduleReconnect { .. })),
            "lagging while Live must not reconnect"
        );
        let sends = take_sends(&actions);
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0].ty, FrameType::RowsReq);
        assert_eq!(
            sends[0].header.get("after").and_then(Value::as_i64),
            Some(4)
        );
        assert_eq!(
            sends[0].header.get("excludeOwn").and_then(Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn ack_does_not_advance_cursor() {
        let pending = vec![("batch-1".into(), b"mine".to_vec())];
        let mut s = RoomSession::new("dev:me", "proj/chat", 20, pending);
        s.on_open(1_000);
        let _ = s.drain_actions();
        s.on_frame(&state_frame(0, Vec::new()));
        let _ = s.drain_actions();
        assert!(s.is_live());
        assert_eq!(s.cursor(), 20);

        s.on_frame(&ack_frame("batch-1", 51));
        let ack_actions = s.drain_actions();
        assert!(s.pending().is_empty());
        assert!(ack_actions
            .iter()
            .any(|a| matches!(a, RoomAction::Pending(p) if p.is_empty())));
        assert!(ack_actions
            .iter()
            .any(|a| matches!(a, RoomAction::Ack { seq: 51, .. })));
        assert!(
            !ack_actions
                .iter()
                .any(|a| matches!(a, RoomAction::Cursor(_))),
            "ACK must not emit Cursor"
        );
        assert_eq!(s.cursor(), 20);

        s.on_frame(&row_frame(21, b"row-21".to_vec()));
        let row_actions = s.drain_actions();
        assert!(row_actions
            .iter()
            .any(|a| matches!(a, RoomAction::Cursor(21))));
        assert_eq!(s.cursor(), 21);
    }

    #[test]
    fn lagging_after_ack_rows_req_keeps_pre_ack_cursor() {
        let pending = vec![("batch-1".into(), b"mine".to_vec())];
        let mut s = RoomSession::new("dev:me", "proj/chat", 20, pending);
        s.on_open(1_000);
        let _ = s.drain_actions();
        s.on_frame(&state_frame(0, Vec::new()));
        let _ = s.drain_actions();
        s.on_frame(&ack_frame("batch-1", 51));
        let _ = s.drain_actions();
        assert_eq!(s.cursor(), 20);

        s.on_frame(&lagging_frame());
        let actions = s.drain_actions();
        let sends = take_sends(&actions);
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0].ty, FrameType::RowsReq);
        assert_eq!(
            sends[0].header.get("after").and_then(Value::as_i64),
            Some(20),
            "lagging RowsReq must use cursor before the ACK seq"
        );
    }

    #[test]
    fn apply_failed_reconnects_from_last_good_cursor() {
        let mut s = RoomSession::new("dev:me", "proj/chat", 20, Vec::new());
        s.on_open(1_000);
        let _ = s.drain_actions();
        s.on_frame(&state_frame(0, Vec::new()));
        let _ = s.drain_actions();
        assert!(s.is_live());
        assert_eq!(s.cursor(), 20);

        s.apply_failed();
        let actions = s.drain_actions();
        assert!(
            !actions.iter().any(|a| matches!(a, RoomAction::Cursor(_))),
            "apply_failed must not emit Cursor"
        );
        assert!(actions.iter().any(|a| matches!(
            a,
            RoomAction::ScheduleReconnect { after } if *after == Duration::ZERO
        )));
        assert!(s.is_closed());
        assert_eq!(s.cursor(), 20);

        s.prepare_reconnect();
        s.on_open(5_000);
        let _ = s.drain_actions();
        s.on_frame(&state_frame(0, Vec::new()));
        let after = s.drain_actions();
        let rows: Vec<_> = take_sends(&after)
            .into_iter()
            .filter(|f| f.ty == FrameType::RowsReq)
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].header.get("after").and_then(Value::as_i64),
            Some(20)
        );
    }

    #[test]
    fn lagging_while_wait_state_ignored_then_one_rows_req() {
        let mut s = RoomSession::new("dev:me", "proj/chat", 0, Vec::new());
        s.on_open(1_000);
        let _ = s.drain_actions();
        // Mid-handshake: lagging must not reconnect or send ROWS_REQ.
        s.on_frame(&lagging_frame());
        let mid = s.drain_actions();
        assert!(mid.is_empty(), "lagging in WaitState is ignored");
        assert!(!s.is_live());
        assert!(!mid
            .iter()
            .any(|a| matches!(a, RoomAction::ScheduleReconnect { .. })));

        s.on_frame(&state_frame(0, Vec::new()));
        let after = s.drain_actions();
        assert!(s.is_live());
        let rows: Vec<_> = take_sends(&after)
            .into_iter()
            .filter(|f| f.ty == FrameType::RowsReq)
            .collect();
        assert_eq!(rows.len(), 1, "finish_handshake sends one ROWS_REQ");
        assert_eq!(
            rows[0].header.get("excludeOwn").and_then(Value::as_bool),
            Some(false)
        );
        assert!(
            !after
                .iter()
                .any(|a| matches!(a, RoomAction::ScheduleReconnect { .. })),
            "no reconnect after lagging-then-STATE"
        );
    }

    #[test]
    fn wake_and_presence_held_while_not_live_flush_after_handshake() {
        let pending = vec![("batch-held".into(), b"pending-bytes".to_vec())];
        let mut s = RoomSession::new("dev:me", "proj/chat", 0, pending);
        s.on_open(1_000);
        let _ = s.drain_actions();

        // Mid-handshake (WaitState): hold, do not send yet.
        s.wake("msg");
        s.presence(json!({ "device": "dev:me", "at": 1 }));
        assert!(s.drain_actions().is_empty());

        // Across close + prepare_reconnect: still held (latest only).
        s.wake("send");
        s.on_close(2_000);
        let _ = s.drain_actions();
        s.prepare_reconnect();
        s.presence(json!({ "device": "dev:me", "at": 2, "focused_chat": "c1" }));

        s.on_open(5_000);
        let _ = s.drain_actions();
        s.on_frame(&state_frame(0, Vec::new()));
        let after = s.drain_actions();
        let sends = take_sends(&after);

        let rows_i = sends.iter().position(|f| f.ty == FrameType::RowsReq);
        let wake_i = sends.iter().position(|f| f.ty == FrameType::Wake);
        let presence_i = sends.iter().position(|f| f.ty == FrameType::Presence);
        assert!(rows_i.is_some(), "RowsReq after handshake");
        assert!(
            sends.iter().all(|f| f.ty != FrameType::Push),
            "pending PUSH held until ROWS_DONE"
        );
        assert!(wake_i.is_some(), "held wake flushed");
        assert!(presence_i.is_some(), "held presence flushed");
        assert!(rows_i < wake_i, "wake after RowsReq");
        assert!(wake_i < presence_i, "presence after wake");
        assert_eq!(
            sends[wake_i.unwrap()]
                .header
                .get("reason")
                .and_then(Value::as_str),
            Some("send")
        );
        assert_eq!(
            sends[presence_i.unwrap()]
                .header
                .get("at")
                .and_then(Value::as_i64),
            Some(2)
        );
        // Only one of each (latest held, not a queue).
        assert_eq!(sends.iter().filter(|f| f.ty == FrameType::Wake).count(), 1);
        assert_eq!(
            sends.iter().filter(|f| f.ty == FrameType::Presence).count(),
            1
        );

        s.on_frame(&rows_done(0));
        let pushes: Vec<_> = take_sends(&s.drain_actions())
            .into_iter()
            .filter(|f| f.ty == FrameType::Push)
            .collect();
        assert_eq!(pushes.len(), 1, "pending PUSH after ROWS_DONE");
        assert_eq!(pushes[0].payload.as_ref(), b"pending-bytes");
    }

    fn rows_done_with_vv(head: i64, vv: &[u8]) -> Vec<u8> {
        Frame::new(FrameType::RowsDone, json!({ "headSeq": head }), vv.to_vec())
            .encode()
            .to_vec()
    }

    /// History heal (#1518): ROWS_DONE with the relay's version vector asks the host to compare;
    /// what the host hands back goes out once per connection as a `heal-…` PUSH; an empty
    /// answer still unblocks the held pending resend; a new connection may heal again.
    #[test]
    fn rows_done_vv_asks_host_and_heal_pushes_once_per_connection() {
        let mut s = RoomSession::new("dev:me", "proj/chat", 0, Vec::new());
        s.on_open(1_000);
        let _ = s.drain_actions();
        s.on_frame(&state_frame(0, Vec::new()));
        let _ = s.drain_actions();

        // Plain ROWS_DONE (older relay): no heal check.
        s.on_frame(&rows_done(5));
        assert!(!s
            .drain_actions()
            .iter()
            .any(|a| matches!(a, RoomAction::HealCheck { .. })));

        s.on_frame(&rows_done_with_vv(5, b"relay-vv"));
        let actions = s.drain_actions();
        assert!(actions.iter().any(|a| matches!(a, RoomAction::LinkLive)));
        assert!(actions
            .iter()
            .any(|a| matches!(a, RoomAction::HealCheck { relay_vv } if relay_vv == b"relay-vv")));

        // Host: missing ops → one heal PUSH, persisted as pending with heal- id.
        s.on_heal(b"missing-ops".to_vec());
        let actions = s.drain_actions();
        let sends = take_sends(&actions);
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0].ty, FrameType::Push);
        assert_eq!(
            sends[0].header.get("heal").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(sends[0].payload.as_ref(), b"missing-ops");
        assert_eq!(s.pending().len(), 1);
        assert!(s.pending()[0].0.starts_with(HEAL_BATCH_PREFIX));
        assert!(actions
            .iter()
            .any(|a| matches!(a, RoomAction::Pending(p) if p.len() == 1)));

        // Same connection: a second ROWS_DONE (after `lagging`) asks nothing, a second answer sends nothing.
        s.on_frame(&rows_done_with_vv(6, b"relay-vv-2"));
        assert!(!s
            .drain_actions()
            .iter()
            .any(|a| matches!(a, RoomAction::HealCheck { .. })));
        s.on_heal(b"more".to_vec());
        assert!(take_sends(&s.drain_actions()).is_empty());

        // ACK clears the heal batch like any push.
        let batch = s.pending()[0].0.clone();
        s.on_frame(&ack_frame(&batch, 7));
        let _ = s.drain_actions();
        assert!(s.pending().is_empty());

        // New connection: may heal again.
        s.on_close(2_000);
        let _ = s.drain_actions();
        s.prepare_reconnect();
        s.on_open(3_000);
        let _ = s.drain_actions();
        s.on_frame(&state_frame(0, Vec::new()));
        let _ = s.drain_actions();
        s.on_frame(&rows_done_with_vv(7, b"relay-vv-3"));
        assert!(s
            .drain_actions()
            .iter()
            .any(|a| matches!(a, RoomAction::HealCheck { .. })));
    }

    fn error_frame(code: &str, message: &str, extra: Value) -> Vec<u8> {
        let mut header = json!({ "code": code, "message": message });
        if let Some(obj) = extra.as_object() {
            for (k, v) in obj {
                header[k] = v.clone();
            }
        }
        Frame::new(FrameType::Error, header, Vec::new())
            .encode()
            .to_vec()
    }

    #[test]
    fn heal_batch_resent_after_reconnect_keeps_heal_true() {
        let pending = vec![(format!("{HEAL_BATCH_PREFIX}old"), b"heal-ops".to_vec())];
        let mut s = RoomSession::new("dev:me", "proj/chat", 0, pending);
        s.on_open(1_000);
        let _ = s.drain_actions();
        s.on_frame(&state_frame(0, Vec::new()));
        let _ = s.drain_actions();
        s.on_frame(&rows_done(0));
        let pushes: Vec<_> = take_sends(&s.drain_actions())
            .into_iter()
            .filter(|f| f.ty == FrameType::Push)
            .collect();
        assert_eq!(pushes.len(), 1);
        assert_eq!(
            pushes[0].header.get("heal").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            pushes[0].header.get("batchId").and_then(Value::as_str),
            Some("heal-old")
        );
    }

    #[test]
    fn rows_done_with_vv_holds_pending_until_on_heal_then_heal_first() {
        let pending = vec![("batch-local".into(), b"local".to_vec())];
        let mut s = RoomSession::new("dev:me", "proj/chat", 0, pending);
        s.on_open(1_000);
        let _ = s.drain_actions();
        s.on_frame(&state_frame(0, Vec::new()));
        let _ = s.drain_actions();

        s.on_frame(&rows_done_with_vv(1, b"relay-vv"));
        let actions = s.drain_actions();
        assert!(actions
            .iter()
            .any(|a| matches!(a, RoomAction::HealCheck { .. })));
        assert!(
            take_sends(&actions).iter().all(|f| f.ty != FrameType::Push),
            "nothing from pending until on_heal"
        );

        s.on_heal(b"missing".to_vec());
        let pushes: Vec<_> = take_sends(&s.drain_actions())
            .into_iter()
            .filter(|f| f.ty == FrameType::Push)
            .collect();
        assert_eq!(pushes.len(), 2);
        assert_eq!(
            pushes[0].header.get("heal").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(pushes[0].payload.as_ref(), b"missing");
        assert_eq!(
            pushes[1].header.get("batchId").and_then(Value::as_str),
            Some("batch-local")
        );
        assert_ne!(
            pushes[1].header.get("heal").and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn on_heal_empty_payload_resends_pending() {
        let pending = vec![("batch-a".into(), b"a".to_vec())];
        let mut s = RoomSession::new("dev:me", "proj/chat", 0, pending);
        s.on_open(1_000);
        let _ = s.drain_actions();
        s.on_frame(&state_frame(0, Vec::new()));
        let _ = s.drain_actions();
        s.on_frame(&rows_done_with_vv(1, b"vv"));
        let _ = s.drain_actions();

        s.on_heal(Vec::new());
        let pushes: Vec<_> = take_sends(&s.drain_actions())
            .into_iter()
            .filter(|f| f.ty == FrameType::Push)
            .collect();
        assert_eq!(pushes.len(), 1);
        assert_eq!(pushes[0].payload.as_ref(), b"a");
        assert!(s
            .pending()
            .iter()
            .all(|(id, _)| !id.starts_with(HEAL_BATCH_PREFIX)));
    }

    #[test]
    fn queuing_a_heal_drops_older_heal_batches() {
        let pending = vec![
            (format!("{HEAL_BATCH_PREFIX}old"), b"old-heal".to_vec()),
            ("batch-local".into(), b"local".to_vec()),
        ];
        let mut s = RoomSession::new("dev:me", "proj/chat", 0, pending);
        s.on_open(1_000);
        let _ = s.drain_actions();
        s.on_frame(&state_frame(0, Vec::new()));
        let _ = s.drain_actions();
        s.on_frame(&rows_done_with_vv(1, b"vv"));
        let _ = s.drain_actions();

        s.on_heal(b"new-heal".to_vec());
        let _ = s.drain_actions();
        assert_eq!(s.pending().len(), 2);
        assert!(s.pending()[0].0.starts_with(HEAL_BATCH_PREFIX));
        assert_eq!(s.pending()[0].1, b"new-heal");
        assert_eq!(s.pending()[1].0, "batch-local");
        assert!(!s.pending().iter().any(|(id, _)| id == "heal-old"));
    }

    #[test]
    fn rate_limited_stays_live_and_resends_once_after_retry_s() {
        let pending = vec![("batch-1".into(), b"x".to_vec())];
        let mut s = RoomSession::new("dev:me", "proj/chat", 0, pending);
        s.on_open(1_000);
        let _ = s.drain_actions();
        s.on_frame(&state_frame(0, Vec::new()));
        let _ = s.drain_actions();
        s.on_frame(&rows_done(0));
        let _ = s.drain_actions();

        s.on_frame(&error_frame(
            "rate_limited",
            "slow down",
            json!({ "batchId": "batch-1", "retryS": 2 }),
        ));
        let after_err = s.drain_actions();
        assert!(s.is_live());
        assert!(!after_err
            .iter()
            .any(|a| matches!(a, RoomAction::ScheduleReconnect { .. })));
        assert!(take_sends(&after_err).is_empty());
        assert_eq!(s.pending().len(), 1);

        // Before retryS: nothing.
        s.on_tick(1_000 + 1_999);
        assert!(take_sends(&s.drain_actions()).is_empty());

        // At retryS: one resend.
        s.on_tick(1_000 + 2_000);
        let pushes: Vec<_> = take_sends(&s.drain_actions())
            .into_iter()
            .filter(|f| f.ty == FrameType::Push)
            .collect();
        assert_eq!(pushes.len(), 1);
        assert_eq!(pushes[0].payload.as_ref(), b"x");
        assert!(s.is_live());
    }

    #[test]
    fn several_rate_limited_errors_schedule_one_resend() {
        let pending = vec![("batch-1".into(), b"x".to_vec())];
        let mut s = RoomSession::new("dev:me", "proj/chat", 0, pending);
        s.on_open(1_000);
        let _ = s.drain_actions();
        s.on_frame(&state_frame(0, Vec::new()));
        let _ = s.drain_actions();
        s.on_frame(&rows_done(0));
        let _ = s.drain_actions();

        s.on_frame(&error_frame("rate_limited", "slow", json!({ "retryS": 5 })));
        let _ = s.drain_actions();
        s.on_frame(&error_frame("rate_limited", "slow", json!({ "retryS": 1 })));
        let _ = s.drain_actions();
        s.on_tick(1_000 + 4_999);
        assert!(take_sends(&s.drain_actions()).is_empty());
        s.on_tick(1_000 + 5_000);
        let pushes: Vec<_> = take_sends(&s.drain_actions())
            .into_iter()
            .filter(|f| f.ty == FrameType::Push)
            .collect();
        assert_eq!(pushes.len(), 1, "one timer, one resend");
    }

    #[test]
    fn not_available_queues_local_commits_and_retries_without_reconnect() {
        let mut s = RoomSession::new("dev:me", "proj/chat", 0, Vec::new());
        s.on_open(1_000);
        let _ = s.drain_actions();
        s.on_frame(&state_frame(0, Vec::new()));
        let _ = s.drain_actions();
        s.on_frame(&rows_done(0));
        let _ = s.drain_actions();

        s.on_frame(&error_frame(
            "not_available",
            "this chat is not available",
            json!({ "batchId": "b1" }),
        ));
        let actions = s.drain_actions();
        assert!(s.is_live());
        assert!(!actions
            .iter()
            .any(|a| matches!(a, RoomAction::ScheduleReconnect { .. })));
        assert!(actions.iter().any(|a| matches!(
            a,
            RoomAction::PushRefused {
                batch_id: Some(id),
                code,
                reason: None,
                ..
            } if id == "b1" && code == "not_available"
        )));
        assert!(actions.iter().any(|a| matches!(
            a,
            RoomAction::SessionError { code, .. } if code == "not_available"
        )));

        // Local commits stay in pending and emit Pending, but send no PUSH.
        s.on_local_commit(b"held".to_vec());
        let after = s.drain_actions();
        assert!(after
            .iter()
            .any(|a| matches!(a, RoomAction::Pending(p) if p.len() == 1)));
        assert!(take_sends(&after).iter().all(|f| f.ty != FrameType::Push));
        assert_eq!(s.pending().len(), 1);
        assert!(!after
            .iter()
            .any(|a| matches!(a, RoomAction::ScheduleReconnect { .. })));

        // Before 60 s: nothing. At 60 s: one pending resend; next wait doubles to 120 s.
        s.on_tick(1_000 + 59_999);
        assert!(take_sends(&s.drain_actions())
            .iter()
            .all(|f| f.ty != FrameType::Push));
        s.on_tick(1_000 + 60_000);
        let pushes: Vec<_> = take_sends(&s.drain_actions())
            .into_iter()
            .filter(|f| f.ty == FrameType::Push)
            .collect();
        assert_eq!(pushes.len(), 1, "one timed resend after 60 s");
        assert_eq!(pushes[0].payload.as_ref(), b"held");

        // Refused again: next wait is 120 s (doubled), capped at 10 min.
        s.on_frame(&error_frame(
            "not_available",
            "still not available",
            json!({ "batchId": "b1" }),
        ));
        let _ = s.drain_actions();
        s.on_tick(1_000 + 60_000 + 119_999);
        assert!(take_sends(&s.drain_actions())
            .iter()
            .all(|f| f.ty != FrameType::Push));
        s.on_tick(1_000 + 60_000 + 120_000);
        let pushes: Vec<_> = take_sends(&s.drain_actions())
            .into_iter()
            .filter(|f| f.ty == FrameType::Push)
            .collect();
        assert_eq!(pushes.len(), 1, "second resend after 120 s");

        // An ACK clears the refusal so the next local commit is pushed at once.
        s.on_frame(&ack_frame(&s.pending()[0].0, 1));
        let _ = s.drain_actions();
        assert!(s.pending().is_empty());
        s.on_local_commit(b"now".to_vec());
        let pushes: Vec<_> = take_sends(&s.drain_actions())
            .into_iter()
            .filter(|f| f.ty == FrameType::Push)
            .collect();
        assert_eq!(pushes.len(), 1);
        assert_eq!(pushes[0].payload.as_ref(), b"now");
    }

    /// A block refusal ([feedback blocked-state-relay-2026-10-26]) is still `not_available`
    /// on the wire (live, no redial, pending kept) and hands the host the reason and the
    /// other party for its sentence.
    #[test]
    fn a_block_refusal_stays_not_available_and_names_the_other_party() {
        let mut s = RoomSession::new("dev:me", "proj/chat", 0, Vec::new());
        s.on_open(1_000);
        let _ = s.drain_actions();
        s.on_frame(&state_frame(0, Vec::new()));
        let _ = s.drain_actions();
        s.on_frame(&rows_done(0));
        let _ = s.drain_actions();

        s.on_frame(&error_frame(
            "not_available",
            "someone in this chat has blocked you",
            json!({ "batchId": "b1", "reason": "blocked_by", "byHandle": "sami", "byName": "Sami Kassab" }),
        ));
        let actions = s.drain_actions();
        assert!(s.is_live());
        assert!(s.push_refused);
        assert!(!actions
            .iter()
            .any(|a| matches!(a, RoomAction::ScheduleReconnect { .. })));
        assert!(actions.iter().any(|a| matches!(
            a,
            RoomAction::PushRefused {
                batch_id: Some(id),
                code,
                reason: Some(reason),
                by_handle: Some(handle),
                by_name: Some(name),
            } if id == "b1" && code == "not_available" && reason == "blocked_by" && handle == "sami" && name == "Sami Kassab"
        )));
        s.on_local_commit(b"held".to_vec());
        let after = s.drain_actions();
        assert!(take_sends(&after).iter().all(|f| f.ty != FrameType::Push));
        assert_eq!(s.pending().len(), 1);
    }

    #[test]
    fn not_available_clears_on_reconnect_so_heal_and_pending_resend() {
        let pending = vec![("batch-1".into(), b"x".to_vec())];
        let mut s = RoomSession::new("dev:me", "proj/chat", 0, pending);
        s.on_open(1_000);
        let _ = s.drain_actions();
        s.on_frame(&state_frame(0, Vec::new()));
        let _ = s.drain_actions();
        s.on_frame(&rows_done_with_vv(0, b"relay-vv"));
        let actions = s.drain_actions();
        assert!(actions
            .iter()
            .any(|a| matches!(a, RoomAction::HealCheck { .. })));

        s.on_heal(b"heal-ops".to_vec());
        let _ = s.drain_actions();

        s.on_frame(&error_frame(
            "not_available",
            "this chat is not available",
            json!({ "batchId": "batch-1" }),
        ));
        let _ = s.drain_actions();
        assert!(s.push_refused);

        // Transport close (not a refusal redial): next connection clears refusal.
        s.on_close(2_000);
        let _ = s.drain_actions();
        s.prepare_reconnect();
        assert!(!s.push_refused);

        s.on_open(3_000);
        let _ = s.drain_actions();
        s.on_frame(&state_frame(0, Vec::new()));
        let _ = s.drain_actions();
        s.on_frame(&rows_done_with_vv(0, b"relay-vv-2"));
        let actions = s.drain_actions();
        assert!(actions
            .iter()
            .any(|a| matches!(a, RoomAction::HealCheck { .. })));
        s.on_heal(Vec::new());
        let pushes: Vec<_> = take_sends(&s.drain_actions())
            .into_iter()
            .filter(|f| f.ty == FrameType::Push)
            .collect();
        assert!(
            !pushes.is_empty(),
            "pending resend goes out again on the new connection"
        );
    }

    /// A parked row (deps missing) never moves the persisted cursor past itself: a relaunch
    /// would lose Loro's in-memory pending ops and the row would never be asked for again
    /// (the per-device holes behind the Mac-vs-iPhone divergence, Oct 23). The hole is
    /// re-requested on reconnect; a clean catch-up or a covering checkpoint releases it.
    #[test]
    fn pending_row_holds_the_persisted_cursor_until_the_hole_is_covered() {
        let mut s = RoomSession::new("dev:me", "proj/chat", 10, Vec::new());
        s.on_open(1_000);
        let _ = s.drain_actions();
        s.on_frame(&state_frame(0, Vec::new()));
        let _ = s.drain_actions();

        s.on_frame(&row_frame(11, b"ok".to_vec()));
        let _ = s.drain_actions();
        assert_eq!(s.cursor(), 11);
        // Row 12 is parked by the host's Loro import.
        s.on_frame(&row_frame(12, b"parked".to_vec()));
        let a = s.drain_actions();
        assert!(
            a.iter().any(|x| matches!(x, RoomAction::Cursor(12))),
            "tentative"
        );
        s.apply_pending();
        let a = s.drain_actions();
        assert!(
            a.iter().any(|x| matches!(x, RoomAction::Cursor(11))),
            "corrected: {a:?}"
        );
        assert_eq!(s.cursor(), 11);
        assert_eq!(s.first_pending_seq(), Some(12));
        // Later rows apply, but the persisted cursor stays below the hole.
        s.on_frame(&row_frame(13, b"ok".to_vec()));
        let a = s.drain_actions();
        assert!(a.iter().any(|x| matches!(x, RoomAction::Cursor(11))));
        s.on_frame(&rows_done(13));
        let a = s.drain_actions();
        assert!(a.iter().any(|x| matches!(x, RoomAction::Cursor(11))));
        assert_eq!(s.cursor(), 11);

        // Reconnect: HELLO and ROWS_REQ resume from below the hole.
        s.on_close(2_000);
        let _ = s.drain_actions();
        s.prepare_reconnect();
        s.on_open(3_000);
        let sends = take_sends(&s.drain_actions());
        assert_eq!(
            sends[0].header.get("cursor").and_then(Value::as_i64),
            Some(11)
        );
        s.on_frame(&state_frame(0, Vec::new()));
        let sends = take_sends(&s.drain_actions());
        let req = sends.iter().find(|f| f.ty == FrameType::RowsReq).unwrap();
        assert_eq!(req.header.get("after").and_then(Value::as_i64), Some(11));
        // This time rows 12 and 13 apply cleanly (their deps arrived): the hole is released.
        s.on_frame(&row_frame(12, b"ok".to_vec()));
        s.on_frame(&row_frame(13, b"ok".to_vec()));
        let _ = s.drain_actions();
        s.on_frame(&rows_done(13));
        let a = s.drain_actions();
        assert!(a.iter().any(|x| matches!(x, RoomAction::Cursor(13))));
        assert_eq!(s.cursor(), 13);
        assert_eq!(s.first_pending_seq(), None);
    }

    #[test]
    fn checkpoint_covering_the_hole_releases_it() {
        let mut s = RoomSession::new("dev:me", "proj/chat", 10, Vec::new());
        s.on_open(1_000);
        let _ = s.drain_actions();
        s.on_frame(&state_frame(0, Vec::new()));
        let _ = s.drain_actions();
        s.on_frame(&row_frame(12, b"parked".to_vec()));
        let _ = s.drain_actions();
        s.apply_pending();
        let _ = s.drain_actions();
        assert_eq!(s.cursor(), 11);
        s.on_close(2_000);
        let _ = s.drain_actions();
        s.prepare_reconnect();
        s.on_open(3_000);
        let _ = s.drain_actions();
        // The relay now has a checkpoint at 20: a whole doc up to there.
        s.on_frame(&state_frame(20, b"frontier".to_vec()));
        let _ = s.drain_actions();
        s.on_checkpoint(20);
        let a = s.drain_actions();
        assert!(a.iter().any(|x| matches!(x, RoomAction::Cursor(20))));
        assert_eq!(s.cursor(), 20);
        assert_eq!(s.first_pending_seq(), None);
    }

    #[test]
    fn oversize_heal_is_dropped_not_sent() {
        let mut s = RoomSession::new("dev:me", "proj/chat", 0, Vec::new());
        s.on_open(1_000);
        let _ = s.drain_actions();
        s.on_frame(&state_frame(0, Vec::new()));
        let _ = s.drain_actions();
        s.on_heal(vec![0u8; ROW_MAX + 1]);
        let actions = s.drain_actions();
        assert!(take_sends(&actions).is_empty());
        assert!(actions
            .iter()
            .any(|a| matches!(a, RoomAction::DroppedOversize { .. })));
        assert!(s.pending().is_empty());
    }

    #[test]
    fn inbound_presence_passes_header_and_flags_mine() {
        let mut s = RoomSession::new("dev:me", "proj/room", 0, vec![]);
        s.on_open(1_000);
        let _ = s.drain_actions();

        let other = Frame::new(
            FrameType::Presence,
            json!({
                "device": "dev:other",
                "user": "u1",
                "at": 42,
                "handle": "h",
            }),
            Vec::new(),
        )
        .encode()
        .to_vec();
        s.on_frame(&other);
        let actions = s.drain_actions();
        let presence: Vec<_> = actions
            .iter()
            .filter_map(|a| match a {
                RoomAction::Presence {
                    mine,
                    handle,
                    header,
                    ..
                } => Some((*mine, handle.clone(), header.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(presence.len(), 1);
        assert!(!presence[0].0);
        assert_eq!(presence[0].1.as_deref(), Some("h"));
        assert_eq!(
            presence[0].2.get("user").and_then(Value::as_str),
            Some("u1")
        );
        assert_eq!(presence[0].2.get("at").and_then(Value::as_i64), Some(42));

        let mine_frame = Frame::new(
            FrameType::Presence,
            json!({ "device": "dev:me", "at": 1 }),
            Vec::new(),
        )
        .encode()
        .to_vec();
        s.on_frame(&mine_frame);
        let mine_actions = s.drain_actions();
        assert!(
            mine_actions
                .iter()
                .any(|a| matches!(a, RoomAction::Presence { mine: true, .. })),
            "PRESENCE from self must set mine true"
        );
    }
}
