//! `arbos-kernel check <place>`: lint `.arbos/`. Parses every agent's
//! `agent.md`, `plan.jsonl`, `attempts.jsonl`, `transcript.jsonl`,
//! `checkpoints.jsonl`, the place's `focus`, `access.toml`, `secrets.toml`,
//! and `kernel.json`, and says what is wrong and where. Exit 1 on any
//! error; warnings alone exit 0. For a hand that edited the folder, and
//! for the fixture runner before it starts a kernel on an authored state.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use arbos_core::{Agent, Layout, Place, list_agents, notes, subscription};
use serde::Serialize;

pub const USAGE: &str = "arbos-kernel check <place> [--json] [--quiet]";

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    /// `error` or `warning`.
    pub level: &'static str,
    /// The file, relative to the place.
    pub path: String,
    /// 1-based line when it is one line's fault.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    pub what: String,
}

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub place: String,
    pub agents: usize,
    pub findings: Vec<Finding>,
}

impl Report {
    fn error(&mut self, path: impl Into<String>, line: Option<usize>, what: impl Into<String>) {
        self.findings.push(Finding {
            level: "error",
            path: path.into(),
            line,
            what: what.into(),
        });
    }
    fn warn(&mut self, path: impl Into<String>, line: Option<usize>, what: impl Into<String>) {
        self.findings.push(Finding {
            level: "warning",
            path: path.into(),
            line,
            what: what.into(),
        });
    }
    pub fn errors(&self) -> usize {
        self.findings.iter().filter(|f| f.level == "error").count()
    }
}

#[derive(Debug, Clone)]
pub struct Args {
    pub place: PathBuf,
    pub json: bool,
    pub quiet: bool,
}

impl Args {
    pub fn parse(args: impl Iterator<Item = String>) -> Result<Self> {
        let mut out = Self {
            place: std::env::current_dir()?,
            json: false,
            quiet: false,
        };
        let mut place_given = false;
        for a in args {
            match a.as_str() {
                "--json" => out.json = true,
                "--quiet" | "-q" => out.quiet = true,
                other if other.starts_with('-') => bail!("check: unknown flag {other}\n{USAGE}"),
                other if !place_given => {
                    out.place = PathBuf::from(other);
                    place_given = true;
                }
                other => bail!("check: unexpected argument {other}\n{USAGE}"),
            }
        }
        Ok(out)
    }
}

pub fn run(args: Args) -> Result<i32> {
    let place = Place::new(std::fs::canonicalize(&args.place).unwrap_or(args.place.clone()));
    let report = check(&place)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if !args.quiet || report.errors() > 0 {
        for f in &report.findings {
            match f.line {
                Some(n) => println!("{}: {}:{}: {}", f.level, f.path, n, f.what),
                None => println!("{}: {}: {}", f.level, f.path, f.what),
            }
        }
        let warnings = report.findings.len() - report.errors();
        println!(
            "{}: {} agents, {} errors, {} warnings",
            place.path.display(),
            report.agents,
            report.errors(),
            warnings
        );
    }
    Ok(if report.errors() > 0 { 1 } else { 0 })
}

/// Everything the lint finds, without printing.
pub fn check(place: &Place) -> Result<Report> {
    let mut r = Report {
        place: place.path.display().to_string(),
        ..Report::default()
    };
    let arbos = place.arbos();
    if !arbos.is_dir() {
        r.error(".arbos", None, "missing: not an Arbos place");
        return Ok(r);
    }
    let rel = |p: &Path| -> String {
        p.strip_prefix(&place.path)
            .unwrap_or(p)
            .display()
            .to_string()
    };

    // Agents: every folder under agents/ must be an agent.
    let agents_dir = place.agents_dir();
    let mut ids: Vec<String> = Vec::new();
    if agents_dir.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(&agents_dir)?.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let dir = e.path();
            let id = e.file_name().to_string_lossy().into_owned();
            match Agent::load(&dir) {
                Ok(a) => {
                    if a.id.as_str() != id {
                        r.error(
                            rel(&dir.join("agent.md")),
                            None,
                            format!(
                                "agent.md says name {:?} but the folder is {id:?}",
                                a.id.as_str()
                            ),
                        );
                    }
                    if a.model.trim().is_empty() {
                        r.warn(
                            rel(&dir.join("agent.md")),
                            None,
                            "model is empty (use `inherit`)",
                        );
                    }
                    ids.push(id.clone());
                }
                // Root without an agent.md is an authored fixture waiting
                // for bootstrap to write one; anyone else is a broken agent.
                Err(_) if id == arbos_core::ROOT_ID && !dir.join("agent.md").exists() => {
                    r.warn(
                        rel(&dir.join("agent.md")),
                        None,
                        "missing; bootstrap writes it at start",
                    );
                    ids.push(id.clone());
                }
                Err(err) => {
                    r.error(
                        rel(&dir.join("agent.md")),
                        None,
                        format!("does not parse: {err:#}"),
                    );
                }
            }
        }
    } else {
        r.warn(
            "agents",
            None,
            "no agents folder (a fresh place; `bootstrap` makes root)",
        );
    }
    r.agents = ids.len();
    let agents = list_agents(place).unwrap_or_default();
    for a in &agents {
        if let Some(parent) = &a.parent
            && !ids.iter().any(|x| x == parent.as_str())
        {
            r.error(
                format!("agents/{}/agent.md", a.id.as_str()),
                None,
                format!("parent {:?} is not an agent here", parent.as_str()),
            );
        }
        let layout = Layout::new(place, a.id.as_str());
        check_subscriptions(&mut r, &rel(&layout.dir.join("subscriptions")), &layout.dir.join("subscriptions"));
        check_notes(&mut r, &rel(&layout.dir.join("notes.md")), &layout.dir.join("notes.md"));
        if layout.plan_jsonl().exists() {
            r.warn(
                rel(&layout.plan_jsonl()),
                None,
                "plan.jsonl is from a kernel before subscriptions/notes.md; the next kernel start migrates it (nodes → notes.md lines, subscriptions, inbox files)",
            );
        }
        check_jsonl::<arbos_core::Event>(
            &mut r,
            &rel(&layout.transcript()),
            &layout.transcript(),
            "event",
        );
        let cps = layout.dir.join("checkpoints.jsonl");
        check_jsonl::<arbos_engine::git::Checkpoint>(&mut r, &rel(&cps), &cps, "checkpoint");
        // Inbox files: front matter must parse, the kind must be known.
        let inbox_dir = arbos_core::inbox::inbox_dir(place, a.id.as_str());
        for e in std::fs::read_dir(&inbox_dir)
            .into_iter()
            .flatten()
            .flatten()
        {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || !name.ends_with(".md") {
                continue;
            }
            let path = e.path();
            match std::fs::read_to_string(&path)
                .map_err(|e| anyhow::anyhow!("{e}"))
                .and_then(|t| arbos_core::inbox::Message::parse(&t))
            {
                Ok(msg) => {
                    if !matches!(
                        msg.kind.as_str(),
                        "message" | "request" | "brief" | "answer" | "approval" | "wake" | "steer"
                    ) {
                        r.error(rel(&path), None, format!("unknown kind {:?}", msg.kind));
                    }
                    if msg.body.trim().is_empty()
                        && msg.attachments.is_empty()
                        && msg.kind != "wake"
                    {
                        r.warn(rel(&path), None, "empty body");
                    }
                    if msg.sent_ms().is_none() {
                        r.warn(
                            rel(&path),
                            None,
                            format!("sent {:?} is not a time", msg.sent),
                        );
                    }
                }
                Err(e) => r.error(rel(&path), None, format!("not an inbox message: {e:#}")),
            }
        }
        // Turn folders: cause.md is a message, meta.toml is TOML.
        let turns = arbos_core::inbox::turns_dir(place, a.id.as_str());
        for e in std::fs::read_dir(&turns).into_iter().flatten().flatten() {
            let dir = e.path();
            if !dir.is_dir() {
                continue;
            }
            let cause = dir.join("cause.md");
            if cause.exists()
                && let Ok(t) = std::fs::read_to_string(&cause)
                && let Err(e) = arbos_core::inbox::Message::parse(&t)
            {
                r.error(rel(&cause), None, format!("not an inbox message: {e:#}"));
            }
            let meta = dir.join("meta.toml");
            if meta.exists()
                && let Ok(t) = std::fs::read_to_string(&meta)
                && let Err(e) = toml::from_str::<toml::Value>(&t)
            {
                r.error(rel(&meta), None, format!("not TOML: {e}"));
            }
        }
    }

    // focus: names an agent that exists. Read raw — `read_focus` repairs
    // the file, and a lint must not write.
    if let Ok(raw) = std::fs::read_to_string(place.focus_path()) {
        if let Err(e) = arbos_core::files::validate_focus(place, &raw) {
            r.error(".arbos/focus", None, format!("{e:#}"));
        }
    }

    // access.toml, secrets.toml: parse.
    let access = arbos.join("access.toml");
    if access.exists() {
        if let Err(e) = crate::access::Access::load(place) {
            r.error(rel(&access), None, format!("{e:#}"));
        }
    }
    let secrets = arbos.join("secrets.toml");
    if secrets.exists() {
        if let Err(e) = arbos_engine::secrets::Config::load(place.path()) {
            r.error(rel(&secrets), None, format!("{e:#}"));
        }
    }

    // kernel.json: a live kernel, or a stale file.
    let kj = place.kernel_json();
    if kj.exists() {
        match std::fs::read_to_string(&kj)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        {
            Some(v) => {
                let pid = v.get("pid").and_then(|p| p.as_u64()).unwrap_or(0);
                if pid == 0 || !pid_alive(pid as u32) {
                    r.warn(rel(&kj), None, format!("stale: pid {pid} is not running"));
                }
            }
            None => r.error(rel(&kj), None, "does not parse"),
        }
    }
    Ok(r)
}

/// Every `subscriptions/*.toml` must parse and validate; ids must not
/// repeat; a due instant must be an RFC 3339 stamp.
fn check_subscriptions(r: &mut Report, rel: &str, dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut ids = Vec::new();
    let mut paths: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name.starts_with('.') {
            continue;
        }
        let file_rel = format!("{rel}/{name}");
        if p.extension().is_none_or(|x| x != "toml") {
            r.warn(&file_rel, None, "not a .toml file; the watcher ignores it");
            continue;
        }
        match subscription::read(&p) {
            Ok(sub) => {
                if let Err(e) = sub.validate() {
                    r.error(&file_rel, None, format!("{e:#}"));
                }
                if ids.contains(&sub.id) {
                    r.error(&file_rel, None, format!("id {} repeats another file's", sub.id));
                }
                ids.push(sub.id);
                if !name.starts_with(&format!("{:04}-", sub.id)) {
                    r.warn(&file_rel, None, format!("file name does not start with {:04}-", sub.id));
                }
                if sub.next_due.is_some() && sub.next_due_ms().is_none() {
                    r.error(&file_rel, None, "next_due is not an RFC 3339 instant");
                }
            }
            Err(e) => r.error(&file_rel, None, format!("{e:#}")),
        }
    }
}

/// `notes.md` is free Markdown; the check says when it holds checkbox
/// lines the parser cannot read (a mark other than space or x).
fn check_notes(r: &mut Report, rel: &str, path: &Path) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    for (i, line) in text.lines().enumerate() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("- [")
            && let Some(end) = rest.find(']')
            && !matches!(&rest[..end], " " | "" | "x" | "X")
        {
            r.warn(rel, Some(i + 1), format!("checkbox mark {:?} is not one the plan tool reads (space or x)", &rest[..end]));
        }
    }
    let n = notes::read_path(path).items().len();
    if n > 200 {
        r.warn(rel, None, format!("{n} items; the prompt shows them all — archive the done ones"));
    }
}

fn check_jsonl<T: serde::de::DeserializeOwned>(r: &mut Report, rel: &str, path: &Path, what: &str) {
    if !path.exists() {
        return;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        r.error(rel, None, "unreadable");
        return;
    };
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        if let Err(e) = serde_json::from_str::<T>(line) {
            r.error(rel, Some(i + 1), format!("not an {what}: {e}"));
        }
    }
}

fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// The lint as the fixture runner uses it: an error list, or nothing.
pub fn problems(place: &Path) -> Result<Vec<String>> {
    let report = check(&Place::new(place.to_path_buf())).context("check")?;
    Ok(report
        .findings
        .iter()
        .filter(|f| f.level == "error")
        .map(|f| match f.line {
            Some(n) => format!("{}:{}: {}", f.path, n, f.what),
            None => format!("{}: {}", f.path, f.what),
        })
        .collect())
}
