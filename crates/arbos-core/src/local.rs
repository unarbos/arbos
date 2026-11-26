//! User-computer access (F9): the shapes `arbos-local` (the helper on the user's Mac),
//! the relay (`WS /local/{deviceId}`, `POST /projects/{id}/local/{device}/exec`) and agentd
//! (`local_run` / `local_read` / `local_list`) agree on. JSON text frames over the WebSocket.

use serde::{Deserialize, Serialize};

/// Largest file the helper returns inline (base64); bigger files go to the relay blob store.
pub const LOCAL_FILE_INLINE_MAX: usize = 1024 * 1024;
/// Per-stream cap on captured stdout / stderr.
pub const LOCAL_OUTPUT_MAX: usize = 256 * 1024;
/// Default and ceiling for `run` timeouts.
pub const LOCAL_RUN_TIMEOUT_DEFAULT_S: u64 = 60;
pub const LOCAL_RUN_TIMEOUT_MAX_S: u64 = 600;
/// Entries returned by `list_dir` at most.
pub const LOCAL_LIST_MAX: usize = 2000;

/// One thing the agent wants done on the user's computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum LocalOp {
    Run {
        cmd: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_s: Option<u64>,
    },
    ReadFile {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_bytes: Option<usize>,
    },
    ListDir {
        path: String,
    },
}

impl LocalOp {
    /// Reads never change the machine; the per-device "allow reads without asking" toggle
    /// applies only to these.
    pub fn is_read(&self) -> bool {
        match self {
            LocalOp::Run { .. } => false,
            LocalOp::ReadFile { .. } | LocalOp::ListDir { .. } => true,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            LocalOp::Run { .. } => "run",
            LocalOp::ReadFile { .. } => "read_file",
            LocalOp::ListDir { .. } => "list_dir",
        }
    }

    /// The exact command or path the user is asked to approve.
    pub fn subject(&self) -> String {
        match self {
            LocalOp::Run { cmd, cwd, .. } => match cwd {
                Some(d) if !d.is_empty() => format!("{cmd}  (in {d})"),
                Some(_) | None => cmd.clone(),
            },
            LocalOp::ReadFile { path, .. } | LocalOp::ListDir { path } => path.clone(),
        }
    }

    /// Wall-clock budget the relay waits for this op.
    pub fn timeout_s(&self) -> u64 {
        match self {
            LocalOp::Run { timeout_s, .. } => timeout_s
                .unwrap_or(LOCAL_RUN_TIMEOUT_DEFAULT_S)
                .clamp(1, LOCAL_RUN_TIMEOUT_MAX_S),
            LocalOp::ReadFile { .. } | LocalOp::ListDir { .. } => 30,
        }
    }
}

/// Helper → relay, first frame after the socket opens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalHello {
    pub hostname: String,
    /// `macos | linux | windows | …` (`std::env::consts::OS`).
    pub os: String,
    pub home: String,
    #[serde(default)]
    pub version: String,
}

/// Relay → helper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalExecRequest {
    pub id: String,
    /// Project that asked; the helper uploads oversized files under this project's blobs.
    pub project_id: String,
    #[serde(flatten)]
    pub op: LocalOp,
}

/// One `list_dir` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalDirEntry {
    pub name: String,
    /// `file | dir | symlink | other`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_ms: Option<i64>,
}

/// Helper → relay → agentd. Exactly one variant per op; `error` for anything that failed
/// before the op could run (bad path, timeout, refused).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LocalResult {
    Run {
        exit: Option<i32>,
        stdout: String,
        stderr: String,
        #[serde(default)]
        stdout_truncated: bool,
        #[serde(default)]
        stderr_truncated: bool,
        #[serde(default)]
        timed_out: bool,
        duration_ms: u64,
    },
    File {
        path: String,
        size: u64,
        /// UTF-8 text when the file is text and fits inline.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        /// Base64 bytes when the file is binary and fits inline.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bytes_b64: Option<String>,
        /// Relay blob key (`GET /blob/{project}/{key}`) when the file exceeded the inline cap.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blob: Option<String>,
        #[serde(default)]
        truncated: bool,
    },
    Dir {
        path: String,
        entries: Vec<LocalDirEntry>,
        #[serde(default)]
        truncated: bool,
    },
    Error {
        message: String,
    },
}

/// Frames the helper sends after `hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LocalClientFrame {
    Hello(LocalHello),
    Result { id: String, result: LocalResult },
    Pong,
}

/// Frames the relay sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LocalServerFrame {
    Exec(LocalExecRequest),
    Ping,
}

/// Box → relay, `POST /projects/{id}/local/{device}/requests`: the op a card is about to ask the
/// user to approve. The relay keeps it; what later runs is the relay's copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalRequestCreate {
    pub op: LocalOp,
    pub chat_id: String,
}

/// Relay → box: the id that goes on the card and back in the exec body as `request_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalRequestCreated {
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalDecision {
    Approve,
    Deny,
}

/// App → relay, `POST /local/requests/{id}/decide`: the user's answer, with the op the card
/// showed. The relay accepts it only when that is the op it stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalDecide {
    pub decision: LocalDecision,
    pub op: LocalOp,
}
