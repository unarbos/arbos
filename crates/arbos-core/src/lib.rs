//! Shared Arbos types. No I/O, no GPUI, no tokio: this crate must build on
//! every target the clients and the box use (design §7.2).

pub mod api;
pub mod approval;
pub mod auth;
pub mod avatar;
pub mod chart;
pub mod chart_render;
pub mod doc;
pub mod highlight;
pub mod ids;
pub mod local;
pub mod redact;
pub mod route;
pub mod rpc;
pub mod search;
pub mod settings_search;
pub mod skill;
pub mod wav;
pub mod widget;
pub mod wire;

/// Protocol version carried in `HELLO` (crdt-sync §7.2) and in the agentd ↔ relay handshake (D44).
pub const PROTO_VERSION: u32 = 1;

/// Streaming commit cadence for LoroText appends (design §4.3).
pub const STREAM_COMMIT_MS: u64 = 120;

/// Inline cap for tool output carried in the doc; the rest goes by `outputRef` (crdt-sync §6.2).
pub const TOOL_OUTPUT_SUMMARY_MAX: usize = 160;

/// Default main-turn wall-clock deadline in seconds. Override at startup with `ARBOS_TURN_DEADLINE_SECS`.
pub const TURN_DEADLINE_SECS: u64 = 900;

/// Offload threshold for the main agent's tool results (design §4.4.1).
pub const OFFLOAD_CHARS: usize = 4_000;

/// `asked_by` a box sends when a human row in a shared chat names no account it can resolve
/// (no `sender: user:<id>`, unknown device, no matching handle). Never a real user id and never
/// the owner's, so the relay's secrets checks treat the asker as a person on no People list,
/// and an owner's "Always allow" for such an ask puts nobody on the list (#1580).
pub const UNKNOWN_ASKER: &str = "unknown";
