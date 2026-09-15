//! Rollout bundles: one agent's run as a folder that can be read, shared,
//! and run again without a model.
//!
//! `arbos-kernel rollout export <place> [--agent ID] [--out DIR]` copies the
//! agent's files (`agent.md`, `transcript.jsonl`, `plan.jsonl`,
//! `feedback.jsonl`, `trace/`) beside a `meta.json` and a `replies.jsonl`:
//! the model's steps as a script for [`arbos_engine::replay`]. With a trace
//! the script is exact; without one it is read off the transcript, one
//! tool call per step.
//!
//! `arbos-kernel rollout replay <bundle> [--out DIR]` builds a fresh place,
//! copies the agent's `agent.md`, sends the bundle's first user prompt
//! through the replay provider, and reports how many tool calls came out
//! the same (name and arguments, in order).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use arbos_core::{Event, EventKind, Layout, Place, load_transcript};
use arbos_engine::replay::{Reply, ReplyCall};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const USAGE: &str = "arbos-kernel rollout export <place> [--agent ID] [--out DIR]\narbos-kernel rollout replay <bundle> [--out DIR] [--timeout SECS]";

/// What `meta.json` records about the run.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Meta {
    pub place: String,
    pub agent: String,
    pub model: String,
    pub kernel_version: String,
    pub kernel_git: String,
    pub exported_ms: i64,
    pub first_ms: Option<i64>,
    pub last_ms: Option<i64>,
    pub events: usize,
    pub tool_calls: usize,
    pub turns: usize,
    /// `trace` or `transcript`: where `replies.jsonl` came from.
    pub replies_from: String,
    pub replies: usize,
}

#[derive(Debug, Clone)]
pub struct Args {
    pub verb: String,
    pub target: PathBuf,
    pub agent: String,
    pub out: Option<PathBuf>,
    pub timeout: Option<u64>,
}

impl Args {
    pub fn parse(mut args: impl Iterator<Item = String>) -> Result<Self> {
        let verb = args.next().context(USAGE)?;
        let mut target = None;
        let mut agent = "root".to_string();
        let mut out = None;
        let mut timeout = None;
        while let Some(a) = args.next() {
            match a.as_str() {
                "--agent" | "-a" => agent = args.next().context("--agent needs an id")?,
                "--out" | "-o" => {
                    out = Some(PathBuf::from(args.next().context("--out needs a dir")?))
                }
                "--timeout" => {
                    timeout = Some(
                        args.next()
                            .context("--timeout needs seconds")?
                            .parse::<u64>()
                            .context("--timeout: not a number")?,
                    )
                }
                other if other.starts_with('-') => bail!("rollout: unknown flag {other}\n{USAGE}"),
                other => target = Some(PathBuf::from(other)),
            }
        }
        let target = target.with_context(|| format!("rollout {verb} needs a path\n{USAGE}"))?;
        Ok(Self {
            verb,
            target,
            agent,
            out,
            timeout,
        })
    }
}

pub fn run(args: Args) -> Result<i32> {
    match args.verb.as_str() {
        "export" => {
            let dir = export(&args.target, &args.agent, args.out.as_deref())?;
            println!("{}", dir.display());
            Ok(0)
        }
        "replay" => replay(&args.target, args.out.as_deref(), args.timeout),
        other => bail!("rollout: unknown verb {other}\n{USAGE}"),
    }
}

/// Write the bundle; returns its folder.
pub fn export(place_path: &Path, agent: &str, out: Option<&Path>) -> Result<PathBuf> {
    let place = Place::new(std::fs::canonicalize(place_path).unwrap_or(place_path.to_path_buf()));
    let layout = Layout::new(&place, agent);
    if !layout.agent_md().exists() {
        bail!("no agent {agent} in {}", place.path.display());
    }
    let events = load_transcript(&layout.transcript()).unwrap_or_default();
    let stamp = stamp(arbos_core::now_ms());
    let root = out
        .map(Path::to_path_buf)
        .unwrap_or_else(|| place.arbos().join("rollouts"));
    let dir = root.join(format!("{stamp}-{}", safe(agent)));
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;

    for (from, to) in [
        (layout.agent_md(), "agent.md"),
        (layout.transcript(), "transcript.jsonl"),
        (layout.dir.join("notes.md"), "notes.md"),
        (layout.instructions(), "instructions.md"),
        (layout.dir.join("feedback.jsonl"), "feedback.jsonl"),
    ] {
        if from.exists() {
            std::fs::copy(&from, dir.join(to))
                .with_context(|| format!("copy {}", from.display()))?;
        }
    }
    let trace_dir = layout.dir.join("trace");
    let mut replies_from = "transcript";
    let mut replies = Vec::new();
    if trace_dir.is_dir() {
        copy_dir(&trace_dir, &dir.join("trace"))?;
        replies = replies_from_trace(&trace_dir, agent)?;
        if !replies.is_empty() {
            replies_from = "trace";
        }
    }
    if replies.is_empty() {
        replies = replies_from_transcript(&events, agent);
    }
    let mut text = String::new();
    for r in &replies {
        text.push_str(&serde_json::to_string(r)?);
        text.push('\n');
    }
    std::fs::write(dir.join("replies.jsonl"), text)?;

    // `inherit` names nothing a reader can act on; the host default it
    // stood for is what ran.
    let model = match arbos_core::Agent::load(&layout.dir).map(|a| a.model) {
        Ok(m) if !m.is_empty() && m != "inherit" => m,
        _ => arbos_core::Host::peek()
            .map(|h| h.config.model())
            .unwrap_or_default(),
    };
    let meta = Meta {
        place: place.path.display().to_string(),
        agent: agent.to_string(),
        model,
        kernel_version: crate::klog::version().to_string(),
        kernel_git: crate::klog::git_sha().to_string(),
        exported_ms: arbos_core::now_ms(),
        first_ms: events.first().map(|e| e.ts).filter(|t| *t > 0),
        last_ms: events.last().map(|e| e.ts).filter(|t| *t > 0),
        events: events.len(),
        tool_calls: events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::Tool(_)))
            .count(),
        turns: events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::TurnComplete { .. }))
            .count(),
        replies_from: replies_from.to_string(),
        replies: replies.len(),
    };
    std::fs::write(dir.join("meta.json"), serde_json::to_string_pretty(&meta)?)?;
    Ok(dir)
}

/// The exact steps: every `turn` trace file in order, its content and calls.
fn replies_from_trace(dir: &Path, agent: &str) -> Result<Vec<Reply>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    let mut out = Vec::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        // Compaction and other side calls are replayed too, in order; the
        // purpose is kept so a reader can tell them apart.
        let purpose = v.get("purpose").and_then(Value::as_str).unwrap_or("turn");
        if v.get("error").is_some_and(|e| !e.is_null()) {
            continue;
        }
        let calls = v
            .get("calls")
            .and_then(Value::as_array)
            .map(|cs| {
                cs.iter()
                    .map(|c| ReplyCall {
                        name: c
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        arguments: c.get("arguments").cloned().unwrap_or(Value::Null),
                        id: c.get("id").and_then(Value::as_str).map(str::to_string),
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.push(Reply {
            content: v
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            calls,
            agent: Some(agent.to_string()),
            source: Some(format!("trace:{purpose}")),
            cost: None,
        });
    }
    Ok(out)
}

/// Steps read off the transcript. The transcript keeps no step
/// boundaries for calls without words, so each call is its own step; the
/// words before a call ride with it.
fn replies_from_transcript(events: &[Event], agent: &str) -> Vec<Reply> {
    let mut out = Vec::new();
    let mut pending: Option<String> = None;
    for e in events {
        match &e.kind {
            EventKind::Assistant { text, .. } => {
                if let Some(prev) = pending.take() {
                    out.push(text_only(prev, agent));
                }
                pending = Some(text.clone());
            }
            EventKind::Tool(rec) => {
                out.push(Reply {
                    content: pending.take().unwrap_or_default(),
                    calls: vec![ReplyCall {
                        name: rec.name.clone(),
                        arguments: rec.args.clone().unwrap_or(Value::Null),
                        id: Some(rec.call_id.clone()),
                    }],
                    agent: Some(agent.to_string()),
                    source: Some("transcript".into()),
                    cost: None,
                });
            }
            EventKind::TurnComplete { .. } | EventKind::User { .. } => {
                if let Some(prev) = pending.take() {
                    out.push(text_only(prev, agent));
                }
            }
            _ => {}
        }
    }
    if let Some(prev) = pending.take() {
        out.push(text_only(prev, agent));
    }
    out
}

fn text_only(content: String, agent: &str) -> Reply {
    Reply {
        content,
        calls: Vec::new(),
        agent: Some(agent.to_string()),
        source: Some("transcript".into()),
        cost: None,
    }
}

/// Run the bundle again in a fresh place and compare the tool calls.
fn replay(bundle: &Path, out: Option<&Path>, timeout: Option<u64>) -> Result<i32> {
    let replies = bundle.join("replies.jsonl");
    if !replies.exists() {
        bail!(
            "{} is not a rollout bundle (no replies.jsonl)",
            bundle.display()
        );
    }
    let recorded = load_transcript(&bundle.join("transcript.jsonl"))
        .with_context(|| format!("read {}/transcript.jsonl", bundle.display()))?;
    let prompt = recorded
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::User { text, .. } => Some(text.clone()),
            _ => None,
        })
        .context("the bundle's transcript has no user line to replay from")?;
    let meta: Meta = std::fs::read_to_string(bundle.join("meta.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let agent = if meta.agent.is_empty() {
        "root".to_string()
    } else {
        meta.agent.clone()
    };

    let place_dir = out.map(Path::to_path_buf).unwrap_or_else(|| {
        std::env::temp_dir().join(format!("arbos-replay-{}", stamp(arbos_core::now_ms())))
    });
    std::fs::create_dir_all(&place_dir)?;
    let place = Place::new(std::fs::canonicalize(&place_dir)?);
    arbos_core::bootstrap(&place)?;
    // The agent's own settings (model name, allowlist, mode) come along;
    // the replayed run is otherwise a clean slate.
    let layout = Layout::new(&place, &agent);
    std::fs::create_dir_all(&layout.dir)?;
    for name in ["agent.md", "instructions.md"] {
        let from = bundle.join(name);
        if from.exists() {
            std::fs::copy(&from, layout.dir.join(name))?;
        }
    }
    // The recorded agent worked in its own place; here it works in this
    // one. A cwd under the old place maps to the same path under the new.
    if let Ok(mut a) = arbos_core::Agent::load(&layout.dir) {
        let old_place = Path::new(&meta.place);
        a.cwd = match a.cwd.take() {
            Some(c) if !meta.place.is_empty() && c.starts_with(old_place) => c
                .strip_prefix(old_place)
                .ok()
                .filter(|rest| !rest.as_os_str().is_empty())
                .map(|rest| place.path.join(rest)),
            Some(c) if c.exists() => Some(c),
            _ => None,
        };
        a.save(&layout.dir)?;
    }

    // The kernel this starts inherits the environment, so the selection
    // reaches its turns.
    arbos_engine::replay::select(&std::fs::canonicalize(&replies)?);
    let code = crate::cli::run(crate::cli::Args {
        place: place.path.clone(),
        agent: agent.clone(),
        json: false,
        steer: false,
        timeout: Some(std::time::Duration::from_secs(timeout.unwrap_or(600))),
        no_spawn: false,
        // A replay has no one at the keyboard.
        no_prompts: true,
        prompt: Some(prompt),
        follow: false,
        allow: None,
        hub: None,
    })?;

    let now = load_transcript(&layout.transcript()).unwrap_or_default();
    let want = calls_of(&recorded);
    let got = calls_of(&now);
    let matched = want
        .iter()
        .zip(got.iter())
        .take_while(|(a, b)| a == b)
        .count();
    println!(
        "replay: {matched}/{} tool calls matched (name+args in order); the run made {}; place {}",
        want.len(),
        got.len(),
        place.path.display()
    );
    if let Some((i, (a, b))) = want
        .iter()
        .zip(got.iter())
        .enumerate()
        .find(|(_, (a, b))| a != b)
    {
        println!(
            "first difference at call {}:\n  recorded {}\n  replayed {}",
            i + 1,
            a,
            b
        );
    }
    if code != 0 {
        return Ok(code);
    }
    Ok(if matched == want.len() && got.len() == want.len() {
        0
    } else {
        2
    })
}

fn calls_of(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::Tool(rec) => Some(format!(
                "{} {}",
                rec.name,
                rec.args.as_ref().map(Value::to_string).unwrap_or_default()
            )),
            _ => None,
        })
        .collect()
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// `20260913T072419Z` from unix millis.
fn stamp(ms: i64) -> String {
    let secs = ms / 1000;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn safe(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
