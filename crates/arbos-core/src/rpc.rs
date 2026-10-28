//! One JSONL codec for pi's RPC mode (`pi --mode rpc`) and Prime Agent's
//! (`prime-agent --mode rpc`), which share the same record shapes
//! (design §4.4.2). Framing: one JSON object per LF-terminated line.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A command written to the child's stdin.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    Prompt {
        id: String,
        message: String,
        #[serde(rename = "streamingBehavior", skip_serializing_if = "Option::is_none")]
        streaming_behavior: Option<StreamingBehavior>,
        /// Image inputs for vision-capable models: `{ "type": "image", "data": base64, "mimeType" }`.
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        images: Vec<Value>,
    },
    Steer {
        id: String,
        message: String,
    },
    FollowUp {
        id: String,
        message: String,
    },
    Abort {
        id: String,
    },
    GetState {
        id: String,
    },
    GetEntries {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        since: Option<String>,
    },
    SwitchSession {
        id: String,
        #[serde(rename = "sessionPath")]
        session_path: String,
    },
    NewSession {
        id: String,
    },
    SetModel {
        id: String,
        provider: String,
        #[serde(rename = "modelId")]
        model_id: String,
    },
    SetThinkingLevel {
        id: String,
        level: String,
    },
    GetLastAssistantText {
        id: String,
    },
    GetMessages {
        id: String,
    },
    /// Compact the live session now (pi keeps `keepRecentTokens` of recent turns and summarises
    /// the rest); `customInstructions` steer the summary.
    Compact {
        id: String,
        #[serde(rename = "customInstructions", skip_serializing_if = "Option::is_none")]
        custom_instructions: Option<String>,
    },
}

impl Command {
    /// The correlation id this command carries.
    pub fn id(&self) -> &str {
        match self {
            Command::Prompt { id, .. }
            | Command::Steer { id, .. }
            | Command::FollowUp { id, .. }
            | Command::Abort { id }
            | Command::GetState { id }
            | Command::GetEntries { id, .. }
            | Command::SwitchSession { id, .. }
            | Command::NewSession { id }
            | Command::SetModel { id, .. }
            | Command::SetThinkingLevel { id, .. }
            | Command::GetLastAssistantText { id }
            | Command::GetMessages { id }
            | Command::Compact { id, .. } => id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StreamingBehavior {
    Steer,
    FollowUp,
}

/// Command response record.
#[derive(Debug, Clone, Deserialize)]
pub struct Response {
    #[serde(default)]
    pub id: Option<String>,
    pub command: String,
    pub success: bool,
    #[serde(default)]
    pub data: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Nested provider event inside `message_update`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantEvent {
    Start,
    TextStart {
        #[serde(rename = "contentIndex")]
        content_index: u32,
    },
    TextDelta {
        #[serde(rename = "contentIndex")]
        content_index: u32,
        delta: String,
    },
    TextEnd {
        #[serde(rename = "contentIndex")]
        content_index: u32,
        content: String,
    },
    ThinkingStart {
        #[serde(rename = "contentIndex")]
        content_index: u32,
    },
    ThinkingDelta {
        #[serde(rename = "contentIndex")]
        content_index: u32,
        delta: String,
    },
    ThinkingEnd {
        #[serde(rename = "contentIndex")]
        content_index: u32,
        content: String,
    },
    ToolcallStart {
        #[serde(rename = "contentIndex")]
        content_index: u32,
        id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
    },
    ToolcallDelta {
        #[serde(rename = "contentIndex")]
        content_index: u32,
        delta: String,
    },
    ToolcallEnd {
        #[serde(rename = "contentIndex")]
        content_index: u32,
        #[serde(rename = "toolCall")]
        tool_call: Value,
    },
    Done {
        #[serde(default)]
        reason: Option<String>,
    },
    Error {
        #[serde(default)]
        reason: Option<String>,
        #[serde(default)]
        error: Option<Value>,
    },
    #[serde(other)]
    Unknown,
}

/// Session event record from the child's stdout.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    AgentStart,
    AgentEnd {
        #[serde(default)]
        messages: Vec<Value>,
        #[serde(rename = "willRetry", default)]
        will_retry: bool,
    },
    AgentSettled {
        #[serde(default)]
        aborted: bool,
    },
    TurnStart,
    TurnEnd {
        #[serde(default)]
        message: Value,
        #[serde(rename = "toolResults", default)]
        tool_results: Vec<Value>,
    },
    MessageStart {
        message: Value,
    },
    MessageUpdate {
        #[serde(default)]
        usage: Option<Usage>,
        #[serde(rename = "assistantMessageEvent")]
        assistant_message_event: AssistantEvent,
    },
    MessageEnd {
        message: Value,
    },
    ToolExecutionStart {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        #[serde(default)]
        args: Value,
        /// Set when another tool issued this call (a `codemode` script): the model-issued call's id.
        #[serde(rename = "parentToolCallId", default)]
        parent_tool_call_id: Option<String>,
    },
    ToolExecutionUpdate {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        #[serde(rename = "partialResult", default)]
        partial_result: Value,
    },
    ToolExecutionEnd {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        #[serde(default)]
        result: Value,
        #[serde(rename = "isError", default)]
        is_error: bool,
        #[serde(rename = "durationMs", default)]
        duration_ms: Option<u64>,
        /// See `ToolExecutionStart::parent_tool_call_id`.
        #[serde(rename = "parentToolCallId", default)]
        parent_tool_call_id: Option<String>,
    },
    QueueUpdate {
        #[serde(default)]
        steering: Vec<Value>,
        #[serde(rename = "followUp", default)]
        follow_up: Vec<Value>,
    },
    EntryAppended {
        entry: Value,
    },
    CompactionStart {
        #[serde(default)]
        reason: Option<String>,
    },
    CompactionEnd {
        #[serde(default)]
        result: Option<Value>,
        #[serde(default)]
        aborted: bool,
        #[serde(rename = "errorMessage", default)]
        error_message: Option<String>,
    },
    /// Extension UI requests (ask_user and friends ride on this record).
    ExtensionUiRequest {
        id: String,
        #[serde(default)]
        method: Option<String>,
        #[serde(flatten)]
        rest: Value,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(rename = "cacheRead", default)]
    pub cache_read: u64,
    #[serde(rename = "cacheWrite", default)]
    pub cache_write: u64,
    #[serde(rename = "totalTokens", default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub cost: Option<Value>,
}

/// One parsed stdout line.
#[derive(Debug, Clone)]
pub enum Record {
    Response(Response),
    Event(Event),
    /// Valid JSON that is neither (kept for logging).
    Raw(Value),
}

/// Parse one JSONL line. Lines that are not JSON return `None` (stderr noise never reaches here, but stay safe).
pub fn parse_line(line: &str) -> Option<Record> {
    let line = line.trim_end_matches('\r');
    if line.is_empty() {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("type").and_then(Value::as_str) == Some("response") {
        return serde_json::from_value::<Response>(v.clone())
            .ok()
            .map(Record::Response)
            .or(Some(Record::Raw(v)));
    }
    match serde_json::from_value::<Event>(v.clone()) {
        Ok(Event::Other) => Some(Record::Raw(v)),
        Ok(e) => Some(Record::Event(e)),
        Err(_) => Some(Record::Raw(v)),
    }
}

/// Serialize a command as one LF-terminated line.
pub fn encode_line(cmd: &Command) -> String {
    let mut s = serde_json::to_string(cmd).expect("command json");
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// pi 1.2 marks a call made by a `codemode` script with `parentToolCallId` (its id is
    /// `<parent>/<n>`); a model-issued call has neither.
    #[test]
    fn nested_tool_events_carry_their_parent() {
        let nested: Event = serde_json::from_str(
            r#"{"type":"tool_execution_start","toolCallId":"toolu_1/2","toolName":"read","args":{"path":"a"},"parentToolCallId":"toolu_1"}"#,
        )
        .unwrap();
        match nested {
            Event::ToolExecutionStart {
                parent_tool_call_id,
                tool_call_id,
                ..
            } => {
                assert_eq!(parent_tool_call_id.as_deref(), Some("toolu_1"));
                assert_eq!(tool_call_id, "toolu_1/2");
            }
            other => panic!("wrong event {other:?}"),
        }
        let direct: Event = serde_json::from_str(
            r#"{"type":"tool_execution_end","toolCallId":"toolu_1","toolName":"codemode","result":{},"isError":false}"#,
        )
        .unwrap();
        match direct {
            Event::ToolExecutionEnd {
                parent_tool_call_id,
                ..
            } => assert!(parent_tool_call_id.is_none()),
            other => panic!("wrong event {other:?}"),
        }
    }
}
