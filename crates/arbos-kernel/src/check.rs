//! `arbos-kernel check <place>`: lint `.arbos/`. Parses every agent's
//! `agent.md`, `plan.jsonl`, `attempts.jsonl`, `transcript.jsonl`,
//! `checkpoints.jsonl`, the place's `focus`, `access.toml`, `secrets.toml`,
//! and `kernel.json`, and says what is wrong and where. Exit 1 on any
//! error; warnings alone exit 0. For a hand that edited the folder, and
//! for the fixture runner before it starts a kernel on an authored state.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use arbos_core::{Agent, Layout, Place, list_agents, node};
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
        check_plan(&mut r, &rel(&layout.plan_jsonl()), &layout.plan_jsonl());
        check_jsonl::<arbos_core::Attempt>(
            &mut r,
            &rel(&layout.attempts_jsonl()),
            &layout.attempts_jsonl(),
            "attempt",
        );
        check_jsonl::<arbos_core::Event>(
            &mut r,
            &rel(&layout.transcript()),
            &layout.transcript(),
            "event",
        );
        let cps = layout.dir.join("checkpoints.jsonl");
        check_jsonl::<arbos_engine::git::Checkpoint>(&mut r, &rel(&cps), &cps, "checkpoint");
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

    // The project's own git must not track .arbos/: the nested repository
    // would be an embedded gitlink, and every turn's commit inside would
    // show up as a modified submodule outside. Jacob's Misc/arbos had 161
    // tracked files there.
    if place.path.join(".git").exists() {
        let out = std::process::Command::new("git")
            .args(["ls-files", "--", ".arbos"])
            .current_dir(&place.path)
            .stdin(std::process::Stdio::null())
            .output();
        if let Ok(out) = out
            && out.status.success()
        {
            let tracked = String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter(|l| !l.trim().is_empty())
                .count();
            if tracked > 0 {
                r.warn(
                    ".arbos",
                    None,
                    format!(
                        "the project's git tracks {tracked} file(s) under .arbos/; run `git rm -r --cached .arbos && echo .arbos/ >> .gitignore` in the project so its history and the agent's record stay apart"
                    ),
                );
            }
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

fn check_plan(r: &mut Report, rel: &str, path: &Path) {
    if !path.exists() {
        return;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        r.error(rel, None, "unreadable");
        return;
    };
    let mut ids = Vec::new();
    let mut parents = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<node::Node>(line) {
            Ok(n) => {
                ids.push(n.id);
                if n.parent != 0 {
                    parents.push((i + 1, n.id, n.parent));
                }
            }
            Err(e) => r.error(rel, Some(i + 1), format!("not a plan node: {e}")),
        }
    }
    for (line, id, parent) in parents {
        if !ids.contains(&parent) {
            r.error(
                rel,
                Some(line),
                format!("node #{id} names parent #{parent}, which is not in the plan"),
            );
        }
    }
    // The folded view the kernel keeps: one line per id at the end.
    match node::load_nodes(path) {
        Ok(_) => {}
        Err(e) => r.error(rel, None, format!("{e:#}")),
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
