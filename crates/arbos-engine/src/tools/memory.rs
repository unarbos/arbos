//! `remember`: durable notes the model keeps for future sessions.
//!
//! The place's memory is one file, `.arbos/memory.md`. The user's store
//! (`~/.config/arbos/`, every place) has Cursor's shape: `preferences.md`
//! is a short index of lasting preferences, each with where it applies;
//! `workflows/<name>.md` are playbooks, `principles/<name>.md` decision
//! rules (with their applicability and stopping boundary), `scripts/<name>`
//! reusable automation. `remember scope=user kind=…` writes them; the
//! prompt shows the index and the names of the rest. `memory.md` in the
//! user dir, from before, is still read.

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
const PREFERENCES_HEADING: &str = "# Preferences";

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

/// `~/.config/arbos/`: the user's store.
pub fn user_dir() -> Option<PathBuf> {
    user_memory().and_then(|p| p.parent().map(Path::to_path_buf))
}

/// `~/.config/arbos/preferences.md`: the index of lasting preferences.
pub fn user_preferences() -> Option<PathBuf> {
    user_dir().map(|d| d.join("preferences.md"))
}

/// What kind of thing a user-store memory is (Cursor's user store).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// One line in `preferences.md`.
    Preference,
    /// `workflows/<name>.md`: a playbook.
    Workflow,
    /// `principles/<name>.md`: a decision rule, where it applies, when to stop.
    Principle,
    /// `scripts/<name>`: reusable automation, made executable.
    Script,
}

impl Kind {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "preference" | "pref" | "fact" => Some(Self::Preference),
            "workflow" | "playbook" => Some(Self::Workflow),
            "principle" | "rule" => Some(Self::Principle),
            "script" => Some(Self::Script),
            _ => None,
        }
    }

    pub fn dir(self) -> &'static str {
        match self {
            Self::Preference => "",
            Self::Workflow => "workflows",
            Self::Principle => "principles",
            Self::Script => "scripts",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Preference => "preference",
            Self::Workflow => "workflow",
            Self::Principle => "principle",
            Self::Script => "script",
        }
    }
}

/// The names under `workflows/`, `principles/`, `scripts/` of the user
/// store, for the prompt: `(kind, name, path)`.
pub fn user_store_index() -> Vec<(&'static str, String, PathBuf)> {
    let Some(dir) = user_dir() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for kind in [Kind::Workflow, Kind::Principle, Kind::Script] {
        let Ok(rd) = std::fs::read_dir(dir.join(kind.dir())) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && !p
                        .file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with('.'))
            })
            .collect();
        paths.sort();
        for p in paths {
            let name = p
                .file_stem()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            out.push((kind.label(), name, p));
        }
    }
    out
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
            "Keep a durable fact. scope place (default): how this project works, a decision and why → .arbos/memory.md. scope user (every place): the user's store — kind preference (default; one line in preferences.md, say where it applies with applies), workflow (a playbook → workflows/<name>.md), principle (a decision rule with its applicability and stopping boundary → principles/<name>.md), script (reusable automation → scripts/<name>, executable). Save a preference only when the user states it, corrects you, or repeats the behaviour; revise a conflicting one (forget then add), never stack. Not progress, never secrets. op forget removes matching lines (or the named file).",
            &[
                (
                    "text",
                    "One self-contained line for a fact or preference; the whole body for a workflow, principle, or script.",
                    true,
                ),
                ("scope", "place (default) or user.", false),
                (
                    "kind",
                    "user scope: preference (default), workflow, principle, or script.",
                    false,
                ),
                (
                    "name",
                    "workflow/principle/script: the file's name (kebab-case).",
                    false,
                ),
                (
                    "applies",
                    "preference: where it applies (\"Python projects\", \"this user's PRs\").",
                    false,
                ),
                ("op", "add (default) or forget.", false),
            ],
        )
    }
    fn plan(&self, cx: &PlanCx, args: &Value) -> Result<Plan> {
        let scope = Scope::parse(str_arg(args, "scope")).unwrap_or(Scope::Place);
        let place = Place::new(cx.root);
        let path = match scope {
            Scope::Place => place_memory(&place),
            Scope::User => user_preferences().unwrap_or_else(|| place_memory(&place)),
        };
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
            if text.is_empty() {
                bail!("remember: text must not be empty");
            }
            let kind_raw = str_arg(&args, "kind");
            let kind = Kind::parse(kind_raw).ok_or_else(|| {
                anyhow::anyhow!(
                    "remember: kind must be preference, workflow, principle, or script, not {kind_raw:?}"
                )
            })?;
            if scope == Scope::Place && kind != Kind::Preference {
                bail!(
                    "remember: kind {} is for scope user (the user's store)",
                    kind.label()
                );
            }
            let path = match (scope, kind) {
                (Scope::Place, _) => place_memory(&cx.place),
                (Scope::User, Kind::Preference) => user_preferences().ok_or_else(|| {
                    anyhow::anyhow!("remember: no home directory for the user store")
                })?,
                (Scope::User, kind) => {
                    let given = str_arg(&args, "name").trim().to_string();
                    // A script keeps its extension (`green.sh`); the rest
                    // are markdown.
                    let (stem, ext) = match kind {
                        Kind::Script => match given.rsplit_once('.') {
                            Some((stem, ext))
                                if !ext.is_empty()
                                    && ext.len() <= 4
                                    && ext.chars().all(|c| c.is_ascii_alphanumeric()) =>
                            {
                                (stem.to_string(), format!(".{ext}"))
                            }
                            _ => (given.clone(), String::new()),
                        },
                        _ => (given.clone(), ".md".to_string()),
                    };
                    let name = if stem.is_empty() {
                        slug(&text)
                    } else {
                        slug(&stem)
                    };
                    if name.is_empty() {
                        bail!("remember: a {} needs a name", kind.label());
                    }
                    let dir = user_dir().ok_or_else(|| {
                        anyhow::anyhow!("remember: no home directory for the user store")
                    })?;
                    dir.join(kind.dir()).join(format!("{name}{ext}"))
                }
            };
            let applies = str_arg(&args, "applies").trim().to_string();
            match (op.as_str(), kind) {
                ("" | "add" | "remember", Kind::Preference) => {
                    let line = if applies.is_empty() {
                        text
                    } else {
                        format!("{text} (applies: {applies})")
                    };
                    add(&path, &line)
                }
                ("" | "add" | "remember", kind) => write_entry(kind, &path, &text),
                ("forget" | "remove" | "delete", Kind::Preference) => forget(&path, &text),
                ("forget" | "remove" | "delete", kind) => forget_entry(kind, &path),
                (other, _) => bail!("remember: op must be add or forget, not {other:?}"),
            }
        })
    }
}

/// A file name from free text: lowercase words joined by `-`, 48 chars.
fn slug(text: &str) -> String {
    let words: Vec<String> = text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    let mut s = String::new();
    for w in words {
        if s.len() + w.len() + 1 > 48 {
            break;
        }
        if !s.is_empty() {
            s.push('-');
        }
        s.push_str(&w);
    }
    s
}

/// A workflow, principle, or script: one file, replaced whole (Cursor's
/// "revise, not stack"), and its line in the `preferences.md` index.
fn write_entry(kind: Kind, path: &Path, text: &str) -> Result<ToolOut> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let existed = path.exists();
    let mut body = text.to_string();
    if !body.ends_with('\n') {
        body.push('\n');
    }
    std::fs::write(path, &body)?;
    #[cfg(unix)]
    if kind == Kind::Script {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
    }
    // The index line: `- workflow: [name](workflows/name.md) — first line`.
    let name = path
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let rel = format!(
        "{}/{}",
        kind.dir(),
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    );
    let first = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .unwrap_or("")
        .chars()
        .take(80)
        .collect::<String>();
    if let Some(index) = user_preferences() {
        let marker = format!("- {}: [{name}]({rel})", kind.label());
        let current = load_for_rewrite(&index)?;
        let kept: Vec<&str> = current
            .lines()
            .filter(|l| !l.trim().starts_with(&marker))
            .collect();
        let mut out = kept.join("\n");
        if out.trim().is_empty() {
            out = format!("{HEADING}\n");
        }
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&format!("{marker} — {first}\n"));
        std::fs::write(&index, out)?;
    }
    Ok(ToolOut {
        body: format!(
            "{} {} {} ({}); indexed in preferences.md",
            if existed { "revised" } else { "saved" },
            kind.label(),
            name,
            path.display()
        ),
        paths: vec![path.display().to_string()],
        child: None,
        images: vec![],
        diff: None,
        park: None,
    })
}

fn forget_entry(kind: Kind, path: &Path) -> Result<ToolOut> {
    if !path.exists() {
        bail!("remember: no {} at {}", kind.label(), path.display());
    }
    std::fs::remove_file(path)?;
    let name = path
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if let Some(index) = user_preferences() {
        let marker = format!("- {}: [{name}](", kind.label());
        let current = load_for_rewrite(&index)?;
        let kept: Vec<&str> = current
            .lines()
            .filter(|l| !l.trim().starts_with(&marker))
            .collect();
        let mut out = kept.join("\n");
        if !out.is_empty() {
            out.push('\n');
        }
        std::fs::write(&index, out)?;
    }
    Ok(ToolOut::text(format!(
        "forgot {} {name} ({})",
        kind.label(),
        path.display()
    )))
}

fn str_arg<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or("")
}

/// One line per memory. Newlines inside a fact become spaces so `forget`
/// and the prompt both see whole facts.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The file's text for display: empty when absent or unreadable.
pub fn load(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// The file's text for a rewrite: empty when absent, an error when the
/// read failed — `remember` used to rewrite `memory.md` from an empty
/// read, wiping every memory on one transient EIO (the qal-j08 family).
fn load_for_rewrite(path: &Path) -> Result<String> {
    Ok(arbos_core::record::read_text(path)
        .confirmed()
        .map_err(|e| anyhow::anyhow!("remember: {e}"))?
        .unwrap_or_default())
}

fn memory_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(str::trim)
        .filter(|l| l.starts_with("- ") || l.starts_with("* "))
}

fn add(path: &Path, text: &str) -> Result<ToolOut> {
    let fact = one_line(text);
    let current = load_for_rewrite(path)?;
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
        let heading = if path.file_name().is_some_and(|n| n == "preferences.md") {
            PREFERENCES_HEADING
        } else {
            HEADING
        };
        out = format!("{heading}\n\n");
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
        park: None,
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
        park: None,
    })
}
