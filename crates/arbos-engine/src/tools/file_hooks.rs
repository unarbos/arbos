//! User hooks around tool calls.
//!
//! Two ways to declare one. Executables under `<place>/.arbos/hooks/<event>/`
//! (a file or a directory of files, name order) match every tool. Entries
//! in `<place>/.arbos/hooks.toml` name an event, a tool-name pattern, and a
//! program:
//!
//! ```toml
//! [[hook]]
//! event = "before-tool"        # before-tool | after-tool | after-turn
//! match = "bash|write|edit"    # globs, `|`-separated; default "*"
//! run = "hooks/guard.sh"       # relative to the place, or absolute
//! timeout_secs = 8
//! ```
//!
//! stdin is one JSON object: `event`, `agent`, `tool`, `args`, and for
//! after-tool a `result` (`body` capped, `error`, `paths`).
//!
//! before-tool exit codes: 0 allow, 2 block (stderr is the reason the model
//! reads), 3 ask the user (stderr is the question), anything else allow
//! with a notice. Timeout blocks. stdout may be a JSON object with `tool`
//! and/or `args` (rewrite), `decision` (`allow|block|ask`), `reason`, and
//! `context` (text added to the tool result for the model).
//!
//! after-tool: stdout `context` is appended to the result body; exit codes
//! are noted, never fatal. after-turn: fire and forget.

use anyhow::{Result, bail};
use arbos_core::{Agent, Place};
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(8);
/// Longest result body an after-tool hook is shown.
const RESULT_CAP: usize = 8 * 1024;

pub const EXIT_BLOCK: i32 = 2;
pub const EXIT_ASK: i32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    BeforeTool,
    AfterTool,
    AfterTurn,
}

impl Event {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BeforeTool => "before-tool",
            Self::AfterTool => "after-tool",
            Self::AfterTurn => "after-turn",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "before-tool" | "before_tool" | "pre-tool" | "PreToolUse" => Some(Self::BeforeTool),
            "after-tool" | "after_tool" | "post-tool" | "PostToolUse" => Some(Self::AfterTool),
            "after-turn" | "after_turn" | "Stop" => Some(Self::AfterTurn),
            _ => None,
        }
    }
}

/// What before-tool hooks decided for one call.
pub struct Decision {
    pub tool: String,
    pub args: Value,
    /// A question for the user before the tool runs; deny = tool error.
    pub ask: Option<String>,
    /// Text the model sees after the tool's result.
    pub context: Vec<String>,
    /// Non-fatal trouble (a hook that failed, bad config), for a notice.
    pub notices: Vec<String>,
}

/// What after-tool hooks added.
#[derive(Default)]
pub struct AfterTool {
    pub context: Vec<String>,
    pub notices: Vec<String>,
}

pub fn before_tool(place: &Place, agent: &Agent, tool: &str, args: &Value) -> Result<Decision> {
    let mut decision = Decision {
        tool: tool.to_string(),
        args: args.clone(),
        ask: None,
        context: Vec::new(),
        notices: Vec::new(),
    };
    let (hooks, mut notices) = hooks_for(place, Event::BeforeTool, tool);
    decision.notices.append(&mut notices);
    for hook in hooks {
        let payload = json!({
            "event": "before-tool",
            "agent": agent.id.as_str(),
            "tool": decision.tool,
            "args": decision.args,
        });
        let out = match run(&hook, place.path(), &payload, agent, Some(&decision.tool)) {
            Ok(out) => out,
            Err(e) => bail!("hook {} blocked {}: {e}", hook.label(), decision.tool),
        };
        let stdout = parse_stdout(&out.stdout);
        let verdict = stdout
            .as_ref()
            .and_then(|o| o.get("decision").and_then(Value::as_str))
            .and_then(Verdict::parse)
            .unwrap_or_else(|| Verdict::from_exit(out.code));
        let reason = stdout
            .as_ref()
            .and_then(|o| o.get("reason").and_then(Value::as_str))
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| out.stderr.trim().to_string());
        match verdict {
            Verdict::Block => {
                if reason.is_empty() {
                    bail!("hook {} blocked {}", hook.label(), decision.tool);
                }
                bail!("hook {} blocked {}: {reason}", hook.label(), decision.tool);
            }
            Verdict::Ask => {
                let q = if reason.is_empty() {
                    format!("hook {} asks: allow {}?", hook.label(), decision.tool)
                } else {
                    reason
                };
                // Several asks fold into one question.
                decision.ask = Some(match decision.ask.take() {
                    Some(prev) => format!("{prev}\n{q}"),
                    None => q,
                });
            }
            Verdict::Allow => {}
            Verdict::Failed(code) => decision.notices.push(format!(
                "hook {} exited {code} on {}{}; the call ran",
                hook.label(),
                decision.tool,
                if reason.is_empty() {
                    String::new()
                } else {
                    format!(": {reason}")
                }
            )),
        }
        if let Some(o) = &stdout {
            apply_rewrite(&mut decision, o);
            if let Some(c) = o.get("context").and_then(Value::as_str) {
                if !c.trim().is_empty() {
                    decision.context.push(c.trim().to_string());
                }
            }
        }
    }
    Ok(decision)
}

pub fn after_tool(
    place: &Place,
    agent: &Agent,
    tool: &str,
    args: &Value,
    body: &str,
    error: Option<&str>,
    paths: &[String],
) -> AfterTool {
    let mut out = AfterTool::default();
    let (hooks, mut notices) = hooks_for(place, Event::AfterTool, tool);
    out.notices.append(&mut notices);
    if hooks.is_empty() {
        return out;
    }
    let shown: String = body.chars().take(RESULT_CAP).collect();
    let payload = json!({
        "event": "after-tool",
        "agent": agent.id.as_str(),
        "tool": tool,
        "args": args,
        "result": { "body": shown, "error": error, "paths": paths },
    });
    for hook in hooks {
        match run(&hook, place.path(), &payload, agent, Some(tool)) {
            Ok(r) => {
                if let Some(o) = parse_stdout(&r.stdout) {
                    if let Some(c) = o.get("context").and_then(Value::as_str) {
                        if !c.trim().is_empty() {
                            out.context.push(c.trim().to_string());
                        }
                    }
                }
                if r.code != 0 {
                    out.notices.push(format!(
                        "hook {} exited {} after {tool}: {}",
                        hook.label(),
                        r.code,
                        r.stderr.trim()
                    ));
                }
            }
            Err(e) => out
                .notices
                .push(format!("hook {} after {tool}: {e}", hook.label())),
        }
    }
    out
}

pub fn after_turn(place: &Place, agent: &Agent) {
    let payload = json!({
        "event": "after-turn",
        "agent": agent.id.as_str(),
    });
    let (hooks, _) = hooks_for(place, Event::AfterTurn, "");
    for hook in hooks {
        let _ = run(&hook, place.path(), &payload, agent, None);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Allow,
    Block,
    Ask,
    Failed(i32),
}

impl Verdict {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "allow" | "approve" | "ok" => Some(Self::Allow),
            "block" | "deny" => Some(Self::Block),
            "ask" => Some(Self::Ask),
            _ => None,
        }
    }
    fn from_exit(code: i32) -> Self {
        match code {
            0 => Self::Allow,
            EXIT_BLOCK => Self::Block,
            EXIT_ASK => Self::Ask,
            other => Self::Failed(other),
        }
    }
}

/// One hook to run: a program and its time cap.
#[derive(Debug, Clone)]
struct Hook {
    program: PathBuf,
    timeout: Duration,
}

impl Hook {
    fn label(&self) -> String {
        self.program
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("hook")
            .to_string()
    }
}

#[derive(Deserialize)]
struct HooksFile {
    #[serde(default)]
    hook: Vec<HookEntry>,
}

#[derive(Deserialize)]
struct HookEntry {
    event: String,
    #[serde(default = "star")]
    #[serde(rename = "match")]
    matches: String,
    run: String,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

fn star() -> String {
    "*".into()
}

/// The hooks for `event` that match `tool`, in order: the `hooks/<event>/`
/// executables first, then `hooks.toml` entries in file order. Config
/// trouble comes back as notices rather than stopping the call.
fn hooks_for(place: &Place, event: Event, tool: &str) -> (Vec<Hook>, Vec<String>) {
    let mut hooks: Vec<Hook> = scripts(place, event.as_str())
        .into_iter()
        .map(|program| Hook {
            program,
            timeout: DEFAULT_TIMEOUT,
        })
        .collect();
    let mut notices = Vec::new();
    let file = place.arbos().join("hooks.toml");
    if let Ok(text) = std::fs::read_to_string(&file) {
        match toml::from_str::<HooksFile>(&text) {
            Ok(parsed) => {
                for entry in parsed.hook {
                    let Some(ev) = Event::parse(&entry.event) else {
                        notices.push(format!(
                            "hooks.toml: unknown event {:?} (use before-tool, after-tool, after-turn)",
                            entry.event
                        ));
                        continue;
                    };
                    if ev != event || !matches_tool(&entry.matches, tool) {
                        continue;
                    }
                    let program = {
                        let p = Path::new(&entry.run);
                        if p.is_absolute() {
                            p.to_path_buf()
                        } else {
                            place.arbos().join(p)
                        }
                    };
                    if !program.is_file() {
                        notices.push(format!(
                            "hooks.toml: {} is not a file; entry skipped",
                            program.display()
                        ));
                        continue;
                    }
                    hooks.push(Hook {
                        program,
                        timeout: entry
                            .timeout_secs
                            .map(Duration::from_secs)
                            .unwrap_or(DEFAULT_TIMEOUT),
                    });
                }
            }
            Err(e) => notices.push(format!("hooks.toml: {e}; only hooks/ dirs ran")),
        }
    }
    (hooks, notices)
}

/// `bash|write|edit`, `*`, `git*`: any pattern matching the tool name.
fn matches_tool(patterns: &str, tool: &str) -> bool {
    patterns
        .split('|')
        .map(str::trim)
        .any(|p| glob_match(p, tool))
}

fn glob_match(pattern: &str, text: &str) -> bool {
    fn go(p: &[u8], t: &[u8]) -> bool {
        match (p.first(), t.first()) {
            (None, None) => true,
            (Some(b'*'), _) => go(&p[1..], t) || (!t.is_empty() && go(p, &t[1..])),
            (Some(b'?'), Some(_)) => go(&p[1..], &t[1..]),
            (Some(a), Some(b)) if a.eq_ignore_ascii_case(b) => go(&p[1..], &t[1..]),
            _ => false,
        }
    }
    go(pattern.as_bytes(), text.as_bytes())
}

fn scripts(place: &Place, event: &str) -> Vec<PathBuf> {
    let root = place.hooks_dir().join(event);
    if !root.exists() {
        return Vec::new();
    }
    if root.is_file() {
        return if is_hook(&root) {
            vec![root]
        } else {
            Vec::new()
        };
    }
    let mut out: Vec<PathBuf> = std::fs::read_dir(&root)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| is_hook(p))
        .collect();
    out.sort();
    out
}

fn is_hook(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    if path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.'))
    {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

struct RunOut {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run(
    hook: &Hook,
    cwd: &Path,
    payload: &Value,
    agent: &Agent,
    tool: Option<&str>,
) -> Result<RunOut> {
    let event = payload
        .get("event")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut cmd = Command::new(&hook.program);
    cmd.current_dir(cwd)
        .env("ARBOS_PLACE", cwd)
        .env("ARBOS_EVENT", &event)
        .env("ARBOS_AGENT", agent.id.as_str());
    if let Some(t) = tool {
        cmd.env("ARBOS_TOOL", t);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("{}: {e}", hook.program.display()))?;
    if let Some(mut stdin) = child.stdin.take() {
        // A hook that never reads stdin must not stall us on a full pipe.
        let _ = stdin.write_all(payload.to_string().as_bytes());
        let _ = stdin.write_all(b"\n");
    }
    let start = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if start.elapsed() > hook.timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("timed out after {:?}", hook.timeout);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output()?;
    Ok(RunOut {
        code: output.status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn parse_stdout(stdout: &str) -> Option<Value> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() || !trimmed.starts_with('{') {
        return None;
    }
    serde_json::from_str::<Value>(trimmed)
        .ok()
        .filter(Value::is_object)
}

fn apply_rewrite(decision: &mut Decision, obj: &Value) {
    if let Some(tool) = obj.get("tool").and_then(|t| t.as_str()) {
        if !tool.is_empty() {
            decision.tool = tool.to_string();
        }
    }
    if let Some(args) = obj.get("args") {
        if args.is_object() {
            decision.args = args.clone();
        }
    }
}
