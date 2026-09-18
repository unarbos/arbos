//! Jev: a cheap structured router that picks the next mechanical move.
//!
//! The configured LLM still writes, plans, and talks. Jev is a decisions
//! model: one `POST /api/alpha/decisions` with `state` + typed questions.
//! It is not a chat-completions model. `act=llm` is a valid pick and runs
//! the chat model. Any failure — error, timeout, or junk — ends the turn.
//! There is no chat-model fallback. Jev does not speak; a tool step it
//! picks looks like any other.

use anyhow::Result;
use arbos_core::{Event, EventKind, Usage, Wake, text};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    batch::{self, BatchCfg, Msg, Outcome},
    control::TurnControl,
    evict,
    provider::{Interrupted, Provider, ProviderError},
    tool::{RunCx, View},
    tools::Hooks,
};

/// OpenRouter family alias (`~typesafe/jev-latest`): always the newest Jev.
pub const DEFAULT_MODEL: &str = arbos_core::host::DEFAULT_JEV_MODEL;
/// Jev's context window. The situation card never exceeds this.
pub const WINDOW_TOKENS: u64 = 32_000;
/// Leave room in the 32k window for the system instruction.
const CARD_TOKEN_BUDGET: u64 = 24_000;
/// Longest wait for Jev's first byte. After this the turn fails in the
/// open. Jev is System One: a healthy call is a few hundred milliseconds.
/// There is no chat-model fallback.
pub const FIRST_BYTE: Duration = Duration::from_millis(1_500);
/// JSON is short; cap the completion so a stall cannot run on.
const MAX_OUTPUT: u64 = 256;
/// Glance size for one tool result in the card.
const TOOL_BODY_CHARS: usize = 400;
/// Goal / last-user clip.
const LINE_CHARS: usize = 800;

/// Tools the user will read: Jev must not write these. The LLM does.
const LLM_TOOLS: &[&str] = &["say", "ask", "plan"];

/// The three acts Jev may return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Tool,
    Llm,
    Done,
}

/// One parsed Jev object.
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub act: Act,
    pub tool: Option<String>,
    pub args: Value,
    pub why: String,
    pub compact: Option<bool>,
    pub fold: Option<bool>,
    pub no_change: bool,
    /// Which chat model the next LLM invoke should use. Ignored on a
    /// pure tool step. Unknown values keep the configured chat model.
    pub model: Option<String>,
    /// Slice ids that stay as text in the standing brief.
    pub keep: Vec<String>,
    /// Slice ids that become addresses only.
    pub pointers: Vec<String>,
}

/// Why asking Jev did not produce a usable decision.
#[derive(Debug)]
pub enum AskError {
    Interrupted,
    Failed(String),
    Junk(String),
}

impl std::fmt::Display for AskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Interrupted => f.write_str("interrupted"),
            Self::Failed(s) | Self::Junk(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for AskError {}

/// What the person reads when Jev fails, times out, or returns junk.
/// `act=llm` never reaches this: that pick is a success.
pub fn fail_notice(e: &AskError) -> String {
    format!("Jev did not choose the next step: {e}. The turn stopped. The chat model did not run.")
}

/// What the turn should do after asking Jev — or without asking.
#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    /// One normal `model_step` with tools.
    Llm,
    /// Run this tool. Do not call the LLM this step.
    Tool { name: String, args: Value },
    /// End the turn. `need_say` when no user-visible sentence exists yet.
    Done { need_say: bool, no_change: bool },
}

/// What the situation card carries: a short view of the turn, never the
/// full transcript, never vault keys, never whole files.
#[derive(Debug, Clone, Default)]
pub struct Situation {
    pub goal: String,
    pub last_user: String,
    pub last_tools: Vec<ToolGlance>,
    pub files_touched: Vec<String>,
    pub tools: Vec<String>,
    pub repro: Option<String>,
    pub first_step: bool,
    pub spoke: bool,
    /// Existing brief glances. Empty when the turn did not gather.
    pub slices: Vec<crate::brief::Slice>,
    /// `fast=…  powerful=…` from the turn's model menu.
    pub model_menu: String,
}

#[derive(Debug, Clone)]
pub struct ToolGlance {
    pub name: String,
    pub args: String,
    pub result: String,
}

/// Whether this host should ask Jev on this turn. Replay stays on the
/// scripted model; `jev = false` is the old loop.
pub fn should_ask(cfg: &arbos_core::HostConfig, has_key: bool, replay: bool) -> bool {
    !replay && cfg.jev_enabled(has_key)
}

/// Parse one Jev reply into a decision. Junk (no JSON, unknown `act`) is
/// an error so the turn fails in the open.
pub fn parse_decision(text: &str) -> Result<Decision, AskError> {
    let value = extract_json(text).ok_or_else(|| AskError::Junk("not a JSON object".into()))?;
    let act_raw = value
        .get("act")
        .or_else(|| value.get("action"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let need_llm = value
        .get("need_llm")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let act = if need_llm {
        Act::Llm
    } else {
        match act_raw.to_ascii_lowercase().as_str() {
            "tool" => Act::Tool,
            "llm" | "need_llm" => Act::Llm,
            "done" => Act::Done,
            other => {
                return Err(AskError::Junk(format!(
                    "unknown act {other:?}; want tool, llm, or done"
                )));
            }
        }
    };
    let tool = value
        .get("tool")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let args = match value.get("args") {
        Some(Value::Object(_)) => value["args"].clone(),
        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or_else(|_| json!({})),
        _ => json!({}),
    };
    let why = value
        .get("why")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let flag = |k: &str| value.get(k).and_then(Value::as_bool);
    let model = value
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Ok(Decision {
        act,
        tool,
        args,
        why,
        compact: flag("compact"),
        fold: flag("fold"),
        no_change: flag("no_change").unwrap_or(false),
        model,
        keep: string_list(&value, "keep"),
        pointers: string_list(&value, "pointers"),
    })
}

fn string_list(value: &Value, key: &str) -> Vec<String> {
    match value.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
        Some(Value::String(s)) => s
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// Turn a decision into a route. Unknown tools, language tools, and a
/// `tool` act with no name become an `llm` step. That is a valid pick,
/// not a fail.
pub fn route(decision: Decision, view: &View, spoke: bool) -> Route {
    match decision.act {
        Act::Llm => Route::Llm,
        Act::Done => Route::Done {
            need_say: !spoke,
            no_change: decision.no_change,
        },
        Act::Tool => {
            let Some(name) = decision.tool.filter(|n| view.get(n).is_some()) else {
                return Route::Llm;
            };
            if LLM_TOOLS.contains(&name.as_str()) {
                return Route::Llm;
            }
            if matches!(name.as_str(), "write" | "edit" | "apply_patch")
                && decision.args.get("path").and_then(Value::as_str).is_none()
            {
                return Route::Llm;
            }
            Route::Tool {
                name,
                args: decision.args,
            }
        }
    }
}

/// Build the situation card from the transcript. Kept under Jev's 32k
/// window; tool bodies are glances, never whole files.
pub fn situation_from_events(
    events: &[Event],
    wake: &Wake,
    tools: &[String],
    repro: Option<String>,
) -> Situation {
    let turn_start = events.iter().rposition(Event::is_wake).unwrap_or(0);
    let turn = &events[turn_start..];
    let last_user = events
        .iter()
        .rev()
        .find_map(|e| match &e.kind {
            EventKind::User { text, .. } => Some(text.clone()),
            _ => None,
        })
        .or_else(|| wake.text.clone())
        .unwrap_or_default();
    let goal = if !wake.brief.trim().is_empty() {
        wake.brief.clone()
    } else {
        last_user.clone()
    };
    let last_tools = events
        .iter()
        .rev()
        .filter_map(|e| match &e.kind {
            EventKind::Tool(rec) => Some(glance_tool(rec)),
            _ => None,
        })
        .take(3)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let mut files_touched = Vec::new();
    for e in turn {
        if let EventKind::Tool(rec) = &e.kind {
            for p in &rec.paths {
                if !p.is_empty() && !files_touched.contains(p) {
                    files_touched.push(p.clone());
                }
            }
        }
    }
    let first_step = !turn.iter().any(|e| matches!(e.kind, EventKind::Tool(_)));
    let spoke = turn.iter().any(|e| match &e.kind {
        EventKind::Assistant { text, .. } => !text.trim().is_empty(),
        _ => false,
    });
    Situation {
        goal,
        last_user,
        last_tools,
        files_touched,
        tools: tools.to_vec(),
        repro,
        first_step,
        spoke,
        slices: Vec::new(),
        model_menu: String::new(),
    }
}

/// The card Jev reads. Always under [`WINDOW_TOKENS`].
pub fn situation_card(sit: &Situation) -> String {
    let mut body_chars = TOOL_BODY_CHARS;
    loop {
        let card = render_card(sit, body_chars);
        if evict::estimate_tokens(&card) <= CARD_TOKEN_BUDGET || body_chars <= 80 {
            return fit_window(card);
        }
        body_chars /= 2;
    }
}

/// True when the card fits Jev's window. Tests pin this.
pub fn card_under_window(card: &str) -> bool {
    evict::estimate_tokens(card) <= WINDOW_TOKENS
}

/// OpenRouter Decisions door. `{api}/v1` → `{api}/alpha/decisions`.
/// Chat completions is the wrong door: Jev 400s there.
pub fn decisions_url(base: &str) -> String {
    let base = base.trim_end_matches('/');
    if let Some(root) = base.strip_suffix("/v1") {
        format!("{root}/alpha/decisions")
    } else {
        format!("{base}/alpha/decisions")
    }
}

/// The official Decisions body: `model` + `state` + typed `questions`.
/// No `messages`. That field is chat-completions and is the 400.
pub fn decisions_body(model: &str, sit: &Situation) -> Value {
    json!({
        "model": model,
        "state": situation_card(sit),
        "questions": controller_questions(sit),
    })
}

fn controller_questions(sit: &Situation) -> Value {
    let mut tools = serde_json::Map::new();
    tools.insert(
        "none".into(),
        json!("No mechanical tool this step; the language model writes."),
    );
    for name in &sit.tools {
        if LLM_TOOLS.contains(&name.as_str()) {
            continue;
        }
        if tools.len() >= 32 {
            break;
        }
        tools.insert(name.clone(), json!(format!("Run the {name} tool.")));
    }
    json!({
        "act": {
            "type": "choice",
            "instructions": "What should the kernel do next? tool = run one listed tool. llm = invoke the language model to write, plan, or talk. done = the turn is finished.",
            "criteria": {
                "tool": "A mechanical read, list, grep, test, or status step.",
                "llm": "Needs language: write, plan, talk, or a step you cannot parse.",
                "done": "The turn is finished."
            }
        },
        "tool": {
            "type": "choice",
            "instructions": "If act is tool, which tool? none if act is llm or done.",
            "criteria": Value::Object(tools)
        },
        "model": {
            "type": "choice",
            "instructions": "If the language model runs, which one?",
            "criteria": {
                "fast": "Cheap and short.",
                "powerful": "Hard coding.",
                "default": "The configured chat model."
            }
        },
        "need_llm": {
            "type": "noul",
            "instructions": "Does this step need the language model regardless of act?",
            "criteria": {
                "true": "Language, planning, or a step no tool can finish.",
                "false": "A listed tool is enough."
            }
        },
        "no_change": {
            "type": "noul",
            "instructions": "Is the tree already what the request asks?",
            "criteria": {
                "true": "A recorded repro now passes and the tree matches.",
                "false": "Work remains."
            }
        },
        "compact": {
            "type": "noul",
            "instructions": "Should the kernel compact context after this step?",
            "criteria": { "true": "Yes, compact.", "false": "No." }
        },
        "fold": {
            "type": "noul",
            "instructions": "Should the kernel fold old tool results after this step?",
            "criteria": { "true": "Yes, fold.", "false": "No." }
        }
    })
}

/// Read a Decisions `answers` map into the same Decision the turn already
/// routes. Missing `act` is junk. `need_llm` ≥ 0.5 is `act=llm`.
pub fn parse_answers(value: &Value) -> Result<Decision, AskError> {
    let answers = value
        .get("answers")
        .and_then(Value::as_object)
        .ok_or_else(|| AskError::Junk("decisions reply has no answers".into()))?;
    let need_llm = noul_of(answers.get("need_llm")).unwrap_or(0.0) >= 0.5;
    let act_raw = choice_of(answers.get("act")).unwrap_or("");
    let act = if need_llm {
        Act::Llm
    } else {
        match act_raw {
            "tool" => Act::Tool,
            "llm" => Act::Llm,
            "done" => Act::Done,
            other => {
                return Err(AskError::Junk(format!(
                    "unknown act {other:?}; want tool, llm, or done"
                )));
            }
        }
    };
    let tool = choice_of(answers.get("tool"))
        .filter(|s| !s.is_empty() && *s != "none")
        .map(str::to_string);
    Ok(Decision {
        act,
        tool,
        args: json!({}),
        why: String::new(),
        compact: noul_flag(answers.get("compact")),
        fold: noul_flag(answers.get("fold")),
        no_change: noul_of(answers.get("no_change")).unwrap_or(0.0) >= 0.5,
        model: choice_of(answers.get("model"))
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        keep: Vec::new(),
        pointers: Vec::new(),
    })
}

fn choice_of(v: Option<&Value>) -> Option<&str> {
    v.and_then(|a| a.get("choice")).and_then(Value::as_str)
}

fn noul_of(v: Option<&Value>) -> Option<f64> {
    v.and_then(|a| a.get("noul")).and_then(Value::as_f64)
}

fn noul_flag(v: Option<&Value>) -> Option<bool> {
    noul_of(v).map(|p| p >= 0.5)
}

fn usage_from_decisions(value: &Value) -> Option<Usage> {
    let u = value.get("usage")?;
    let used = u
        .get("input_tokens")
        .or_else(|| u.get("prompt_tokens"))
        .and_then(Value::as_u64)?;
    Some(Usage {
        used,
        size: WINDOW_TOKENS,
        cost: u.get("cost").and_then(Value::as_f64),
        cached: None,
    })
}

/// Ask Jev for one decision. The caller ends the turn on Failed or Junk.
/// Interrupted is barge-in. The slug without `~` is remapped so a saved
/// old default does not 400. The call is Decisions, not chat completions.
pub async fn ask(
    src: &Provider,
    model: &str,
    sit: &Situation,
    cancel: &CancellationToken,
    hooks: &dyn Hooks,
) -> Result<(Decision, Option<Usage>), AskError> {
    // A step a person reads under the shimmer — not the router's name
    // (the desktop showed "Working jev" on every ordinary turn).
    hooks.kernel_step("Choosing the next step");
    let model = arbos_core::host::normalize_jev_slug(model);
    let model = if model.is_empty() {
        DEFAULT_MODEL
    } else {
        model
    };
    let jev = Provider {
        base: src.base.clone(),
        key: src.key.clone(),
        model: model.to_string(),
        reasoning_effort: None,
        cache_ttl: None,
        data_policy: src.data_policy.clone(),
        stream_idle: FIRST_BYTE,
        first_byte: FIRST_BYTE,
        max_tokens: Some(MAX_OUTPUT),
        trace: src.trace.clone(),
        trace_agent: src.trace_agent.clone(),
        trace_purpose: "jev".into(),
        trace_line: src.trace_line,
        replay: None,
    };
    let url = decisions_url(&jev.base);
    let body = decisions_body(model, sit);
    let replied = jev
        .post_json(url, body, cancel, |for_| hooks.working(for_.as_secs()))
        .await;
    let value = match replied {
        Ok(v) => v,
        Err(e) if e.is::<Interrupted>() => return Err(AskError::Interrupted),
        Err(e) => {
            let why = e
                .downcast_ref::<ProviderError>()
                .map(|pe| pe.to_string())
                .unwrap_or_else(|| format!("{e:#}"));
            return Err(AskError::Failed(why));
        }
    };
    let decision = parse_answers(&value)?;
    Ok((decision, usage_from_decisions(&value)))
}

/// Run one Jev-chosen tool through the same batch path a model step uses.
pub async fn run_tool(
    view: &View,
    cx: &RunCx,
    control: &TurnControl,
    batch_cfg: BatchCfg,
    name: String,
    args: Value,
) -> Result<(
    Vec<crate::provider::ToolCall>,
    Vec<(crate::provider::ToolCall, Outcome)>,
)> {
    let call = crate::provider::ToolCall {
        id: format!("jev-{}", cx.step),
        name,
        arguments: args,
    };
    let (tx, rx) = mpsc::unbounded_channel();
    let executor = tokio::spawn(batch::run(
        view.clone(),
        cx.clone(),
        control.clone(),
        batch_cfg,
        rx,
    ));
    let _ = tx.send(Msg::Call(call.clone()));
    let _ = tx.send(Msg::Commit);
    drop(tx);
    let outcomes = executor
        .await
        .map_err(|e| anyhow::anyhow!("jev tool batch: {e}"))?;
    Ok((vec![call], outcomes))
}

fn extract_json(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(trimmed)
        && v.is_object()
    {
        return Some(v);
    }
    let body = if let Some(rest) = trimmed.strip_prefix("```json") {
        rest.strip_suffix("```").unwrap_or(rest)
    } else if let Some(rest) = trimmed.strip_prefix("```") {
        rest.strip_suffix("```").unwrap_or(rest)
    } else {
        trimmed
    };
    if let Ok(v) = serde_json::from_str::<Value>(body.trim())
        && v.is_object()
    {
        return Some(v);
    }
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&trimmed[start..=end])
        .ok()
        .filter(Value::is_object)
}

fn glance_tool(rec: &arbos_core::ToolRec) -> ToolGlance {
    if rec.name == "secret" {
        return ToolGlance {
            name: rec.name.clone(),
            args: "(redacted)".into(),
            result: "(secret; not sent)".into(),
        };
    }
    let args = rec
        .args
        .as_ref()
        .map(|a| {
            let mut copy = a.clone();
            if let Some(obj) = copy.as_object_mut() {
                for k in ["contents", "content", "patch", "old_string", "new_string"] {
                    if obj.contains_key(k) {
                        obj.insert(k.into(), json!("(omitted)"));
                    }
                }
            }
            clip_chars(&copy.to_string(), 160)
        })
        .unwrap_or_default();
    let result = if let Some(e) = &rec.error {
        clip_chars(e, TOOL_BODY_CHARS)
    } else {
        clip_chars(rec.body.as_deref().unwrap_or(""), TOOL_BODY_CHARS)
    };
    ToolGlance {
        name: rec.name.clone(),
        args,
        result,
    }
}

fn render_card(sit: &Situation, body_chars: usize) -> String {
    let mut out = String::new();
    out.push_str("goal: ");
    out.push_str(&clip_chars(&sit.goal, LINE_CHARS));
    out.push('\n');
    out.push_str("last_user: ");
    out.push_str(&clip_chars(&sit.last_user, LINE_CHARS));
    out.push('\n');
    out.push_str("first_step: ");
    out.push_str(if sit.first_step { "true" } else { "false" });
    out.push('\n');
    out.push_str("spoke: ");
    out.push_str(if sit.spoke { "true" } else { "false" });
    out.push('\n');
    if let Some(repro) = &sit.repro {
        out.push_str("repro: ");
        out.push_str(&clip_chars(repro, LINE_CHARS));
        out.push('\n');
    }
    out.push_str("files_touched: ");
    if sit.files_touched.is_empty() {
        out.push_str("(none)");
    } else {
        out.push_str(&sit.files_touched.join(", "));
    }
    out.push('\n');
    out.push_str("tools: ");
    out.push_str(&sit.tools.join(", "));
    out.push('\n');
    out.push_str("last_tools:\n");
    if sit.last_tools.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for (i, t) in sit.last_tools.iter().enumerate() {
            out.push_str(&format!(
                "  {}. {} {} → {}\n",
                i + 1,
                t.name,
                t.args,
                clip_chars(&t.result, body_chars)
            ));
        }
    }
    if !sit.slices.is_empty() {
        out.push_str("slices:\n");
        for s in &sit.slices {
            out.push_str(&format!(
                "  {} [{}] {}\n",
                s.id,
                s.kind,
                clip_chars(&s.body, 160)
            ));
        }
    }
    if !sit.model_menu.is_empty() {
        out.push_str("models: ");
        out.push_str(&sit.model_menu);
        out.push('\n');
    }
    out
}

fn fit_window(card: String) -> String {
    if card_under_window(&card) {
        return card;
    }
    let max_chars = (WINDOW_TOKENS.saturating_mul(4) as usize).max(64);
    clip_chars(&card, max_chars)
}

fn clip_chars(s: &str, n: usize) -> String {
    text::clip(s, n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbos_core::{HostConfig, ProviderKind, ToolRec};

    fn tool_view() -> View {
        crate::tool::Registry::builtin().view(&arbos_core::Agent::root("root"))
    }

    #[test]
    fn parse_accepts_the_three_acts() {
        let tool = parse_decision(
            r#"{"act":"tool","tool":"grep","args":{"pattern":"foo"},"why":"find it"}"#,
        )
        .unwrap();
        assert_eq!(tool.act, Act::Tool);
        assert_eq!(tool.tool.as_deref(), Some("grep"));
        assert_eq!(tool.args["pattern"], "foo");
        assert_eq!(tool.why, "find it");

        let llm = parse_decision(r#"{"act":"llm","why":"write the file"}"#).unwrap();
        assert_eq!(llm.act, Act::Llm);

        let done = parse_decision(r#"{"act":"done","why":"tests pass"}"#).unwrap();
        assert_eq!(done.act, Act::Done);
        assert!(!done.no_change);
    }

    #[test]
    fn parse_accepts_fenced_json_and_need_llm() {
        let d = parse_decision("```json\n{\"act\":\"tool\",\"tool\":\"ls\"}\n```").unwrap();
        assert_eq!(d.act, Act::Tool);
        assert_eq!(d.tool.as_deref(), Some("ls"));

        let llm = parse_decision(r#"{"act":"tool","need_llm":true,"why":"prose"}"#).unwrap();
        assert_eq!(llm.act, Act::Llm);

        let alias = parse_decision(r#"{"action":"done","no_change":true}"#).unwrap();
        assert_eq!(alias.act, Act::Done);
        assert!(alias.no_change);
    }

    #[test]
    fn junk_is_rejected() {
        assert!(parse_decision("not json").is_err());
        assert!(parse_decision(r#"{"act":"dance"}"#).is_err());
        assert!(parse_decision("[]").is_err());
        assert!(parse_decision("").is_err());
        match parse_decision("hello") {
            Err(e @ AskError::Junk(_)) => {
                let notice = fail_notice(&e);
                assert!(notice.contains("not a JSON object"), "{notice}");
                assert!(notice.contains("The chat model did not run"), "{notice}");
            }
            other => panic!("expected junk, got {other:?}"),
        }
    }

    #[test]
    fn language_tools_and_unknown_tools_become_llm() {
        let view = tool_view();
        let say = parse_decision(r#"{"act":"tool","tool":"say","args":{"text":"hi"}}"#).unwrap();
        assert_eq!(route(say, &view, false), Route::Llm);

        let missing = parse_decision(r#"{"act":"tool","tool":"nope"}"#).unwrap();
        assert_eq!(route(missing, &view, false), Route::Llm);

        let nameless = parse_decision(r#"{"act":"tool"}"#).unwrap();
        assert_eq!(route(nameless, &view, false), Route::Llm);

        let grep =
            parse_decision(r#"{"act":"tool","tool":"grep","args":{"pattern":"x"}}"#).unwrap();
        assert_eq!(
            route(grep, &view, false),
            Route::Tool {
                name: "grep".into(),
                args: json!({"pattern": "x"}),
            }
        );

        let done = parse_decision(r#"{"act":"done"}"#).unwrap();
        assert_eq!(
            route(done, &view, false),
            Route::Done {
                need_say: true,
                no_change: false
            }
        );
        let done_spoke = parse_decision(r#"{"act":"done"}"#).unwrap();
        assert_eq!(
            route(done_spoke, &view, true),
            Route::Done {
                need_say: false,
                no_change: false
            }
        );
    }

    #[test]
    fn situation_card_stays_under_32k() {
        let huge = "x".repeat(200_000);
        let sit = Situation {
            goal: huge.clone(),
            last_user: huge.clone(),
            last_tools: vec![
                ToolGlance {
                    name: "read".into(),
                    args: huge.clone(),
                    result: huge.clone(),
                },
                ToolGlance {
                    name: "bash".into(),
                    args: huge.clone(),
                    result: huge.clone(),
                },
                ToolGlance {
                    name: "grep".into(),
                    args: huge.clone(),
                    result: huge,
                },
            ],
            files_touched: vec!["a.rs".into(); 200],
            tools: vec!["read".into(), "grep".into(), "bash".into()],
            repro: Some("cargo test -p huge".into()),
            first_step: false,
            spoke: false,
            slices: Vec::new(),
            model_menu: String::new(),
        };
        let card = situation_card(&sit);
        assert!(
            card_under_window(&card),
            "card was {} tokens",
            evict::estimate_tokens(&card)
        );
        assert!(
            !card.contains(&"x".repeat(1_000)),
            "whole files must not ride"
        );
    }

    #[test]
    fn jev_false_is_the_old_loop() {
        let on = HostConfig::default();
        assert!(should_ask(&on, true, false));
        assert!(!should_ask(&on, true, true), "replay never asks Jev");
        let off = HostConfig {
            jev: Some(false),
            ..HostConfig::default()
        };
        assert!(!should_ask(&off, true, false));
        let mut openai = HostConfig::default();
        openai.set_provider(ProviderKind::OpenAi);
        assert!(!should_ask(&openai, true, false));
    }

    #[test]
    fn first_byte_is_a_router_wait_not_a_chat_wait() {
        assert_eq!(FIRST_BYTE, Duration::from_millis(1_500));
        assert!(
            FIRST_BYTE < Duration::from_secs(3),
            "a 15s cap leaves Choosing the next step on the window"
        );
    }

    #[test]
    fn decisions_url_is_the_official_door() {
        assert_eq!(
            decisions_url("https://openrouter.ai/api/v1"),
            "https://openrouter.ai/api/alpha/decisions"
        );
        assert!(!decisions_url("https://openrouter.ai/api/v1").contains("chat/completions"));
    }

    #[test]
    fn decisions_body_is_state_and_questions() {
        let sit = Situation {
            last_user: "what files are in this folder?".into(),
            tools: vec!["ls".into(), "grep".into(), "say".into()],
            first_step: true,
            ..Situation::default()
        };
        let body = decisions_body("~typesafe/jev-latest", &sit);
        assert_eq!(body["model"], "~typesafe/jev-latest");
        assert!(body.get("messages").is_none(), "{body}");
        assert!(
            body["state"]
                .as_str()
                .unwrap_or("")
                .contains("what files are in this folder?"),
            "{body}"
        );
        assert_eq!(body["questions"]["act"]["type"], "choice");
        assert!(body["questions"]["tool"]["criteria"].get("ls").is_some());
        assert!(
            body["questions"]["tool"]["criteria"].get("say").is_none(),
            "say is an LLM tool"
        );
    }

    #[test]
    fn parse_answers_reads_choice_and_noul() {
        let v = json!({
            "answers": {
                "act": {"type": "choice", "choice": "tool"},
                "tool": {"type": "choice", "choice": "ls"},
                "model": {"type": "choice", "choice": "fast"},
                "need_llm": {"type": "noul", "noul": 0.1},
                "no_change": {"type": "noul", "noul": 0.0}
            }
        });
        let d = parse_answers(&v).unwrap();
        assert_eq!(d.act, Act::Tool);
        assert_eq!(d.tool.as_deref(), Some("ls"));
        assert_eq!(d.model.as_deref(), Some("fast"));
        assert!(!d.no_change);

        let llm = parse_answers(&json!({
            "answers": {
                "act": {"type": "choice", "choice": "tool"},
                "need_llm": {"type": "noul", "noul": 0.9}
            }
        }))
        .unwrap();
        assert_eq!(llm.act, Act::Llm);
    }

    #[test]
    fn secret_results_never_enter_the_card() {
        let rec = ToolRec {
            name: "secret".into(),
            call_id: "c".into(),
            step: 1,
            paths: vec![],
            started: None,
            ended: None,
            result_size: None,
            error: None,
            body: Some("op://vault/item/password".into()),
            args: Some(json!({"name": "OPENROUTER_API_KEY"})),
            child: None,
            images: vec![],
            diff: None,
            label: None,
            output: None,
        };
        let g = glance_tool(&rec);
        assert_eq!(g.result, "(secret; not sent)");
        assert!(!g.args.contains("OPENROUTER"));
    }

    #[test]
    fn parse_keeps_act_when_model_is_junk() {
        let d = parse_decision(
            r#"{"act":"llm","model":"not-a-real-slug","keep":["tldr","working"],"pointers":["ask"]}"#,
        )
        .unwrap();
        assert_eq!(d.act, Act::Llm);
        assert_eq!(d.model.as_deref(), Some("not-a-real-slug"));
        assert_eq!(d.keep, vec!["tldr", "working"]);
        assert_eq!(d.pointers, vec!["ask"]);

        let tool = parse_decision(r#"{"act":"tool","tool":"grep","model":"fast"}"#).unwrap();
        assert_eq!(tool.act, Act::Tool);
        assert_eq!(tool.model.as_deref(), Some("fast"));
        assert!(tool.keep.is_empty());
    }
}
