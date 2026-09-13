//! `remember`: durable notes the model keeps for future sessions.
//!
//! Two files, both plain markdown the user may edit: `.arbos/memory.md`
//! for the place, `~/.config/arbos/memory.md` for the user across places.
//! Every instance prompt shows both, so a fact stored once is known in
//! every later session and after every context checkpoint.

use anyhow::{Result, bail};
use serde_json::Value;
use std::path::{Path, PathBuf};

use super::ToolOut;
use crate::access::Access;
use crate::tool::{BoxFuture, Plan, PlanCx, RunCx, Tool, blocking, simple_schema};
use arbos_core::Place;

/// Past this many memory lines, `add` refuses until something is forgotten.
/// A memory file is for what matters; a log grows past what a prompt can
/// carry.
const MAX_LINES: usize = 200;
const HEADING: &str = "# Memory";

pub struct Remember;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Place,
    User,
}

impl Scope {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "place" | "project" | "repo" => Some(Self::Place),
            "user" | "global" | "me" => Some(Self::User),
            _ => None,
        }
    }

    pub fn path(self, place: &Place) -> Option<PathBuf> {
        match self {
            Self::Place => Some(place_memory(place)),
            Self::User => user_memory(),
        }
    }
}

pub fn place_memory(place: &Place) -> PathBuf {
    place.arbos().join("memory.md")
}

pub fn user_memory() -> Option<PathBuf> {
    if let Some(base) = std::env::var_os("XDG_CONFIG_HOME").filter(|b| !b.is_empty()) {
        return Some(PathBuf::from(base).join("arbos").join("memory.md"));
    }
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(|home| {
            PathBuf::from(home)
                .join(".config")
                .join("arbos")
                .join("memory.md")
        })
}

impl Tool for Remember {
    fn name(&self) -> &'static str {
        "remember"
    }
    fn schema(&self) -> Value {
        simple_schema(
            "remember",
            "Keep a fact for future sessions: how this project works, a decision and why, a preference of the user. It lands in .arbos/memory.md (scope place, default) or ~/.config/arbos/memory.md (scope user: true in every place) and shows in your prompt from now on. Not for task progress (that is the plan) and never for secrets. op forget removes the lines that contain text.",
            &[
                ("text", "The fact, one line, self-contained.", true),
                ("scope", "place (default) or user.", false),
                ("op", "add (default) or forget.", false),
            ],
        )
    }
    fn plan(&self, cx: &PlanCx, args: &Value) -> Result<Plan> {
        let scope = Scope::parse(str_arg(args, "scope")).unwrap_or(Scope::Place);
        let place = Place::new(cx.root);
        let path = scope.path(&place).unwrap_or_else(|| place_memory(&place));
        Ok(Plan::access(Access::writes([path])))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        blocking(move || {
            let text = str_arg(&args, "text").trim().to_string();
            let scope_raw = str_arg(&args, "scope");
            let scope = Scope::parse(scope_raw).ok_or_else(|| {
                anyhow::anyhow!("remember: scope must be place or user, not {scope_raw:?}")
            })?;
            let op = str_arg(&args, "op").trim().to_ascii_lowercase();
            let path = scope.path(&cx.place).ok_or_else(|| {
                anyhow::anyhow!("remember: no home directory for the user memory file")
            })?;
            if text.is_empty() {
                bail!("remember: text must not be empty");
            }
            match op.as_str() {
                "" | "add" | "remember" => add(&path, &text),
                "forget" | "remove" | "delete" => forget(&path, &text),
                other => bail!("remember: op must be add or forget, not {other:?}"),
            }
        })
    }
}

fn str_arg<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or("")
}

/// One line per memory. Newlines inside a fact become spaces so `forget`
/// and the prompt both see whole facts.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn load(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn memory_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(str::trim)
        .filter(|l| l.starts_with("- ") || l.starts_with("* "))
}

fn add(path: &Path, text: &str) -> Result<ToolOut> {
    let fact = one_line(text);
    let current = load(path);
    if memory_lines(&current).any(|l| l[2..].trim() == fact) {
        return Ok(ToolOut::text(format!(
            "already remembered ({}): {fact}",
            path.display()
        )));
    }
    let count = memory_lines(&current).count();
    if count >= MAX_LINES {
        bail!(
            "remember: {} already holds {count} memories (cap {MAX_LINES}). Forget what no longer matters (op forget) or consolidate the file with edit before adding more.",
            path.display()
        );
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut out = current;
    if out.trim().is_empty() {
        out = format!("{HEADING}\n\n");
    } else if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("- ");
    out.push_str(&fact);
    out.push('\n');
    std::fs::write(path, out)?;
    Ok(ToolOut {
        body: format!(
            "remembered ({}, {} memories): {fact}",
            path.display(),
            count + 1
        ),
        paths: vec![path.display().to_string()],
        child: None,
        images: vec![],
        diff: None,
    })
}

fn forget(path: &Path, text: &str) -> Result<ToolOut> {
    let needle = one_line(text).to_ascii_lowercase();
    let current = load(path);
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for line in current.lines() {
        let t = line.trim();
        let is_memory = t.starts_with("- ") || t.starts_with("* ");
        if is_memory && one_line(t).to_ascii_lowercase().contains(&needle) {
            dropped.push(t[2..].trim().to_string());
        } else {
            kept.push(line);
        }
    }
    if dropped.is_empty() {
        bail!(
            "remember: nothing in {} contains {text:?}; the memories there:\n{}",
            path.display(),
            memory_lines(&current)
                .map(|l| format!("  {l}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    std::fs::write(path, out)?;
    Ok(ToolOut {
        body: format!(
            "forgot {} ({}):\n{}",
            dropped.len(),
            path.display(),
            dropped
                .iter()
                .map(|d| format!("  - {d}"))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        paths: vec![path.display().to_string()],
        child: None,
        images: vec![],
        diff: None,
    })
}
