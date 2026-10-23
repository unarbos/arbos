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
    /// updates since `relay_vv`; when non-empty, call [`RoomSession::on_heal`] with them —
    /// the relay lost rows this device still holds (#1518) and gets them back as one row.
    HealCheck { relay_vv: Vec<u8> },
    /// Socket died or handshake failed; host should reconnect after `after`.
    ScheduleReconnect { after: Duration },
    /// Hard relay error (not `lagging`); tear down this attempt.
    SessionError { code: String, message: String },
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
    /// A history heal was pushed on this connection (one per connection; reset by
    /// `prepare_reconnect`).
    healed: bool,
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
        }
    }

    pub fn room(&self) -> &str {
        &self.room
    }

    pub fn cursor(&self) -> i64 {
        self.cursor
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
        let hello = Hello {
            cursor: self.cursor,
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
        self.push(RoomAction::Cursor(self.cursor));
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
        self.push(RoomAction::Cursor(self.cursor));
        self.finish_handshake();
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
            after: self.cursor,
            exclude_own: false,
        };
        let header = serde_json::to_value(&req).expect("rows_req");
        self.push(RoomAction::SendFrame(Self::encode(
            FrameType::RowsReq,
            header,
            Vec::new(),
        )));
        let pending = self.pending.clone();
        for (batch_id, bytes) in pending {
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
        // Flush wake / presence held while reconnecting or mid-handshake.
        // After RowsReq and pending PUSH resend so catch-up stays first.
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
        self.push(RoomAction::Cursor(self.cursor));
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
        self.push(RoomAction::Cursor(self.cursor));
        self.push(RoomAction::LinkLive);
        if !frame.payload.is_empty() && !self.healed {
            self.push(RoomAction::HealCheck {
                relay_vv: frame.payload.to_vec(),
            });
        }
    }

    /// Host answer to [`RoomAction::HealCheck`]: `bytes` are the doc's updates the relay's
    /// version vector lacked. Pushed once per connection as a row flagged `heal`, through the
    /// same pending queue as a local commit (persisted, resent, acked). Empty = nothing to do.
    pub fn on_heal(&mut self, bytes: Vec<u8>) {
        if bytes.is_empty() || self.healed || self.phase != Phase::Live {
            return;
        }
        if bytes.len() > ROW_MAX {
            self.push(RoomAction::DroppedOversize { bytes: bytes.len() });
            return;
        }
        self.healed = true;
        let batch_id = ulid();
        self.pending.push((batch_id.clone(), bytes.clone()));
        self.push(RoomAction::Pending(self.pending.clone()));
        let header = serde_json::to_value(PushHeader {
            batch_id,
            heal: true,
        })
        .expect("push");
        self.push(RoomAction::SendFrame(Self::encode(
            FrameType::Push,
            header,
            bytes,
        )));
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
        }
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
                    after: self.cursor,
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
    pub fn on_local_commit(&mut self, bytes: Vec<u8>) {
        if bytes.len() > ROW_MAX {
            self.push(RoomAction::DroppedOversize { bytes: bytes.len() });
            return;
        }
        let batch_id = ulid();
        self.pending.push((batch_id.clone(), bytes.clone()));
        self.push(RoomAction::Pending(self.pending.clone()));
        if self.phase == Phase::Live {
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

    /// Host clock tick: enforce HELLO deadline; mark live-since on first live tick.
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

        // New connection: HELLO, catch-up, then resend pending PUSH.
        s.prepare_reconnect();
        s.on_open(5_000);
        let _ = s.drain_actions();
        s.on_frame(&state_frame(0, Vec::new()));
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
        let push_i = sends.iter().position(|f| f.ty == FrameType::Push);
        let wake_i = sends.iter().position(|f| f.ty == FrameType::Wake);
        let presence_i = sends.iter().position(|f| f.ty == FrameType::Presence);
        assert!(rows_i.is_some(), "RowsReq after handshake");
        assert!(push_i.is_some(), "pending PUSH resent");
        assert!(wake_i.is_some(), "held wake flushed");
        assert!(presence_i.is_some(), "held presence flushed");
        assert!(rows_i < push_i, "PUSH after RowsReq");
        assert!(push_i < wake_i, "wake after pending PUSH");
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
    }

    fn rows_done_with_vv(head: i64, vv: &[u8]) -> Vec<u8> {
        Frame::new(FrameType::RowsDone, json!({ "headSeq": head }), vv.to_vec())
            .encode()
            .to_vec()
    }

    /// History heal (#1518): ROWS_DONE with the relay's version vector asks the host to compare;
    /// what the host hands back goes out once per connection as a `heal` PUSH through the
    /// pending queue; nothing is asked twice, nothing is sent for an empty answer, and a new
    /// connection may heal again.
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

        // Host: nothing missing → nothing sent.
        s.on_heal(Vec::new());
        assert!(s.drain_actions().is_empty());

        // Host: missing ops → one heal PUSH, persisted as pending.
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
