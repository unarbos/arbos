//! File hooks under `<place>/.arbos/hooks/`.
//!
//! Missing dir = nothing runs.
//!
//! `before-tool` — a file, or a directory of files. Each executable runs in
//! name order. stdin is one JSON object: event, agent, tool, args.
//! Exit 0 = allow. Non-zero = the tool does not run (stderr is the reason).
//! If stdout is a JSON object with `tool` and/or `args`, that replaces the call.
//!
//! `after-turn` — same layout. Failure is ignored.

use anyhow::{Result, bail};
use arbos_core::{Agent, Place};
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(8);

pub struct Decision {
    pub tool: String,
    pub args: Value,
}

pub fn before_tool(place: &Place, agent: &Agent, tool: &str, args: &Value) -> Result<Decision> {
    let mut decision = Decision {
        tool: tool.to_string(),
        args: args.clone(),
    };
    for script in scripts(place, "before-tool") {
        let payload = json!({
            "event": "before-tool",
            "agent": agent.id.as_str(),
            "tool": decision.tool,
            "args": decision.args,
        });
        let out = run(
            &script,
            place.path(),
            &payload,
            &[
                ("ARBOS_EVENT", "before-tool"),
                ("ARBOS_TOOL", &decision.tool),
                ("ARBOS_AGENT", agent.id.as_str()),
            ],
        )?;
        if !out.ok {
            let why = out.stderr.trim();
            if why.is_empty() {
                bail!("hook {} denied {}", script.display(), decision.tool);
            }
            bail!("hook {} denied {}: {why}", script.display(), decision.tool);
        }
        apply_rewrite(&mut decision, &out.stdout);
    }
    Ok(decision)
}

pub fn after_turn(place: &Place, agent: &Agent) {
    let payload = json!({
        "event": "after-turn",
        "agent": agent.id.as_str(),
    });
    for script in scripts(place, "after-turn") {
        let _ = run(
            &script,
            place.path(),
            &payload,
            &[
                ("ARBOS_EVENT", "after-turn"),
                ("ARBOS_AGENT", agent.id.as_str()),
            ],
        );
    }
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
    ok: bool,
    stdout: String,
    stderr: String,
}

fn run(script: &Path, cwd: &Path, payload: &Value, env: &[(&str, &str)]) -> Result<RunOut> {
    let mut child = Command::new(script)
        .current_dir(cwd)
        .env("ARBOS_PLACE", cwd)
        .envs(env.iter().map(|(k, v)| (*k, *v)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("hook {}: {e}", script.display()))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(payload.to_string().as_bytes())?;
        stdin.write_all(b"\n")?;
    }
    let start = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if start.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            bail!("hook {} timed out", script.display());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output()?;
    Ok(RunOut {
        ok: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn apply_rewrite(decision: &mut Decision, stdout: &str) {
    let trimmed = stdout.trim();
    if trimmed.is_empty() || !trimmed.starts_with('{') {
        return;
    }
    let Ok(v) = serde_json::from_str::<Value>(trimmed) else {
        return;
    };
    let Some(obj) = v.as_object() else {
        return;
    };
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
