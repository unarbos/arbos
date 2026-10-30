//! Binary room frames (crdt-sync §7.1–7.2):
//! `[type: u8][headerLen: u32 LE][header: JSON ≤ 4096 B][payload: raw bytes]`.

use bytes::{Buf, BufMut, Bytes, BytesMut};
use serde::{Deserialize, Serialize};

pub const HEADER_MAX: usize = 4096;
/// Row cap (crdt-sync §7.6).
pub const ROW_MAX: usize = 1024 * 1024 - 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum FrameType {
    Hello = 0x01,
    State = 0x02,
    RowsReq = 0x03,
    Row = 0x04,
    RowsDone = 0x05,
    Push = 0x06,
    Ack = 0x07,
    Presence = 0x08,
    Probe = 0x09,
    ProbeOk = 0x0a,
    Error = 0x0b,
    Status = 0x0c,
    Wake = 0x0d,
}

impl FrameType {
    pub fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            0x01 => Self::Hello,
            0x02 => Self::State,
            0x03 => Self::RowsReq,
            0x04 => Self::Row,
            0x05 => Self::RowsDone,
            0x06 => Self::Push,
            0x07 => Self::Ack,
            0x08 => Self::Presence,
            0x09 => Self::Probe,
            0x0a => Self::ProbeOk,
            0x0b => Self::Error,
            0x0c => Self::Status,
            0x0d => Self::Wake,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub ty: FrameType,
    pub header: serde_json::Value,
    pub payload: Bytes,
}

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("frame too short")]
    Short,
    #[error("unknown frame type {0:#x}")]
    UnknownType(u8),
    #[error("header too long ({0} > {HEADER_MAX})")]
    HeaderTooLong(usize),
    #[error("header is not a JSON object: {0}")]
    Header(#[from] serde_json::Error),
}

impl Frame {
    pub fn new(ty: FrameType, header: serde_json::Value, payload: impl Into<Bytes>) -> Self {
        Self {
            ty,
            header,
            payload: payload.into(),
        }
    }

    pub fn encode(&self) -> Bytes {
        let header = serde_json::to_vec(&self.header).expect("json header");
        let mut out = BytesMut::with_capacity(5 + header.len() + self.payload.len());
        out.put_u8(self.ty as u8);
        out.put_u32_le(header.len() as u32);
        out.put_slice(&header);
        out.put_slice(&self.payload);
        out.freeze()
    }

    /// Decode one frame. Returns `Ok(None)` for an unknown type so callers skip it (crdt-sync §7.2).
    pub fn decode(mut buf: Bytes) -> Result<Option<Self>, DecodeError> {
        if buf.len() < 5 {
            return Err(DecodeError::Short);
        }
        let ty_byte = buf.get_u8();
        let header_len = buf.get_u32_le() as usize;
        if header_len > HEADER_MAX {
            return Err(DecodeError::HeaderTooLong(header_len));
        }
        if buf.len() < header_len {
            return Err(DecodeError::Short);
        }
        let header_bytes = buf.split_to(header_len);
        let Some(ty) = FrameType::from_u8(ty_byte) else {
            return Ok(None);
        };
        let header: serde_json::Value = serde_json::from_slice(&header_bytes)?;
        Ok(Some(Self {
            ty,
            header,
            payload: buf,
        }))
    }
}

/// `HELLO` header (C→S).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub cursor: i64,
    pub device: String,
    pub room: String,
    pub proto: u32,
}

/// `STATE` header (S→C).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    #[serde(rename = "headSeq")]
    pub head_seq: i64,
    #[serde(rename = "seqFloor")]
    pub seq_floor: i64,
    #[serde(rename = "checkpointSeq")]
    pub checkpoint_seq: i64,
    #[serde(rename = "checkpointSize")]
    pub checkpoint_size: i64,
    #[serde(rename = "rowCount")]
    pub row_count: i64,
    #[serde(rename = "rowBytes")]
    pub row_bytes: i64,
    #[serde(rename = "box")]
    pub box_state: BoxState,
}

/// Box lifecycle as the relay reports it (crdt-sync §8.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoxState {
    Awake,
    Waking,
    Asleep,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowsReq {
    pub after: i64,
    #[serde(rename = "excludeOwn", default)]
    pub exclude_own: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowHeader {
    pub seq: i64,
    pub device: String,
    #[serde(rename = "batchId")]
    pub batch_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushHeader {
    #[serde(rename = "batchId")]
    pub batch_id: String,
    /// History heal: these are ops the device holds and the relay's `ROWS_DONE` version vector
    /// lacked (a lost checkpoint). The relay audits and caps such pushes; the row is otherwise
    /// an ordinary row.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub heal: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ack {
    #[serde(rename = "batchId")]
    pub batch_id: String,
    pub seq: i64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dup: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusHeader {
    #[serde(rename = "box")]
    pub box_state: BoxState,
    pub since: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WakeHeader {
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorHeader {
    pub code: String,
    pub message: String,
    /// Present on people-push refusals (`rate_limited` / `not_available`).
    #[serde(rename = "batchId", default, skip_serializing_if = "Option::is_none")]
    pub batch_id: Option<String>,
    /// Seconds to wait before retrying a `rate_limited` push.
    #[serde(rename = "retryS", default, skip_serializing_if = "Option::is_none")]
    pub retry_s: Option<i64>,
    /// Why a `not_available` push was refused ([feedback blocked-state-relay-2026-10-26]):
    /// `blocked_by` (someone in the chat blocked the sender), `blocks_member` (the sender
    /// blocked the other person of a DM), `silenced`, `chat_unavailable`; a block also names
    /// the other party. The code itself never changes (old clients key on it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(rename = "byHandle", default, skip_serializing_if = "Option::is_none")]
    pub by_handle: Option<String>,
    #[serde(rename = "byName", default, skip_serializing_if = "Option::is_none")]
    pub by_name: Option<String>,
}

/// Room name for a chat doc: `{projectId}/{chatId}`. The project doc uses chat id `project`.
pub fn room_name(project_id: &str, chat_id: &str) -> String {
    format!("{project_id}/{chat_id}")
}

pub const PROJECT_DOC_CHAT_ID: &str = "project";
