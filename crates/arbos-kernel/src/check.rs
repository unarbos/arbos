//! `arbos-kernel check <place>`: lint `.arbos/`. Parses every agent's
//! `agent.md`, `plan.jsonl`, `attempts.jsonl`, `transcript.jsonl`,
//! `checkpoints.jsonl`, the place's `focus`, `notes.md` (the status page's
//! shape), `access.toml`, `secrets.toml`, and `kernel.json`, and says what
//! is wrong and where. Exit 1 on any
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
        check_subscriptions(
            &mut r,
            &rel(&layout.dir.join("subscriptions")),
            &layout.dir.join("subscriptions"),
        );
        if a.id.as_str() != arbos_core::ROOT_ID {
            check_notes(
                &mut r,
                &rel(&layout.dir.join("notes.md")),
                &layout.dir.join("notes.md"),
            );
        } else if layout.dir.join("notes.md").exists() {
            r.warn(
                rel(&layout.dir.join("notes.md")),
                None,
                "root's checklist is the project page .arbos/notes.md; this file is not read",
            );
        }
        check_notes(
            &mut r,
            &rel(&layout.dir.join(arbos_core::notes::TODO)),
            &layout.dir.join(arbos_core::notes::TODO),
        );
        check_waiting(
            &mut r,
            &rel(&layout.dir.join("waiting")),
            &layout.dir.join("waiting"),
        );
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

    // The project page keeps the coordinator protocol's shape (checkbox
    // items led by a link, tldr and completed caps). Warnings: a page that
    // drifts still serves, and the lint says where. `check_notes` below
    // covers what the plan tool's parser can read.
    let notes = arbos_core::store::notes_path(place);
    if let Ok(text) = std::fs::read_to_string(&notes) {
        for p in arbos_core::store::lint_notes(&text) {
            r.warn(rel(&notes), Some(p.line), p.what);
        }
    }
    // Jobs whose command reached for the cloud metadata service, the
    // container runtime, or credential files: named, so a run that was
    // led there is visible after the fact (the guard asked at the time).
    for agent in &agents {
        let root = arbos_engine::JobsRoot::for_agent(place, &agent.id);
        for job in root.list() {
            if let Some(risk) = arbos_core::containment::risk_of(&job.meta.command) {
                r.warn(
                    format!(".arbos/agents/{}/jobs/{}", agent.id, job.id),
                    None,
                    format!(
                        "this job reached for {risk}: {}",
                        arbos_core::text::clip(job.meta.command.trim(), 100)
                    ),
                );
            }
        }
    }
    // results/ grows with every long tool result and nothing prunes it:
    // past 50 MB it is worth a look (delete what is old; archived workers
    // take theirs with them).
    for agent in &agents {
        let dir = place.agent_dir(agent.id.as_str()).join("results");
        let bytes: u64 = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum();
        if bytes > 50 * 1024 * 1024 {
            r.warn(
                format!(".arbos/agents/{}/results", agent.id),
                None,
                format!(
                    "{} MB of spilled tool results; nothing prunes this folder — delete what is old",
                    bytes / (1024 * 1024)
                ),
            );
        }
    }
    // Processes still writing into .arbos/: a job from an earlier kernel
    // run (the kernel reaps these at start; a `check` between runs sees
    // them), and, on Linux, any process holding a file under .arbos/ open
    // for writing that is not this one.
    for agent in &agents {
        let root = arbos_engine::JobsRoot::for_agent(place, &agent.id);
        for (id, pid, command, who) in root.leftovers() {
            let what = match who {
                arbos_engine::PidIdentity::Ours => {
                    "job still running from an earlier kernel run; the kernel ends it at its next start (add a `keep` file to the job folder to spare it)"
                }
                _ => {
                    "a process holds this job's pid and this machine cannot tell whether it is the job; the kernel leaves it running — kill it by hand if it is"
                }
            };
            r.warn(
                format!(".arbos/agents/{}/jobs/{id}", agent.id),
                None,
                format!(
                    "{what} (pid {pid}): {}",
                    arbos_core::text::clip(command.trim(), 80)
                ),
            );
        }
    }
    // Long transcripts and paused agents with timers due in the past: the
    // Mac wake-up incident had a paused agent at 17,969 lines whose timer
    // would have fired a backlog on resume.
    for agent in &agents {
        let transcript = Layout::new(place, agent.id.as_str()).transcript();
        let lines = std::fs::read_to_string(&transcript)
            .map(|t| t.lines().count())
            .unwrap_or(0);
        if lines > 10_000 {
            r.warn(
                rel(&transcript),
                None,
                format!(
                    "{lines} lines; the kernel rolls it into transcript-archive/ at the next turn end (transcript_roll_lines in config.toml, default 10000)"
                ),
            );
        }
        if agent.paused {
            let now = arbos_core::now_ms();
            let overdue = subscription::list(place, agent.id.as_str())
                .into_iter()
                .filter(|s| s.next_due_ms().is_some_and(|d| d <= now))
                .count();
            if overdue > 0 {
                r.warn(
                    format!("agents/{}/subscriptions", agent.id),
                    None,
                    format!(
                        "paused agent has {overdue} subscription(s) due in the past; they are rescheduled one period out on resume (nothing fires while paused)"
                    ),
                );
            }
        }
    }
    for (pid, args, file) in writers_into(place) {
        r.warn(
            rel(&file),
            None,
            format!("open for writing by pid {pid} ({}): a process outside the kernel is changing the store", arbos_core::text::clip(&args, 80)),
        );
    }

    // PROTOCOL.md: the long-form contract the prompt points at. The kernel
    // rewrites it at start; a stale copy means an older kernel wrote it.
    let protocol = arbos_core::protocol::path(place);
    match std::fs::read_to_string(&protocol) {
        Ok(text) if text == arbos_core::protocol::TEXT => {}
        Ok(_) => r.warn(
            rel(&protocol),
            None,
            "differs from this build's protocol text (the kernel rewrites it at start)",
        ),
        Err(_) => r.warn(
            rel(&protocol),
            None,
            "missing: the contract points agents here (the kernel writes it at start)",
        ),
    }
    // GOALS.md: a real file here is the pre-store layout; bootstrap moves
    // it into docs/project-context.md at the next start.
    let goals = arbos_core::store::goals_alias_path(place);
    if std::fs::symlink_metadata(&goals).is_ok_and(|m| m.is_file()) {
        r.warn(
            rel(&goals),
            None,
            "old layout: the context file now lives at docs/project-context.md (bootstrap moves it and leaves this as a symlink)",
        );
    }

    // access.toml, secrets.toml: parse. access.toml also gets the lint a
    // tunnel operator wants before opening the port: no client rows (the
    // kernel would refuse to bind off loopback), a token in a file others
    // can read.
    let access = arbos.join("access.toml");
    if access.exists() {
        match crate::access::Access::load(place) {
            Err(e) => r.error(rel(&access), None, format!("{e:#}")),
            Ok(a) => {
                if !a.has_clients() {
                    r.warn(
                        rel(&access),
                        None,
                        "no [[client]] rows: a kernel bound off loopback refuses to start (fail closed)",
                    );
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(meta) = std::fs::metadata(&access)
                        && meta.permissions().mode() & 0o077 != 0
                        && std::fs::read_to_string(&access).is_ok_and(|t| t.contains("token = "))
                    {
                        r.warn(
                            rel(&access),
                            None,
                            "holds a token but is readable by others; chmod 600",
                        );
                    }
                }
            }
        }
    }
    // The project page: root's checklist.
    check_notes(&mut r, ".arbos/notes.md", &arbos.join("notes.md"));
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

    // A place inside iCloud / a file-provider sync: reads of .arbos/ can
    // block for minutes on dataless items (Jacob's ~/Documents/Misc/arbos).
    // Silent once the store is a symlink out of the sync.
    if let Some(sync) = arbos_core::cloudsync::detect(place.path())
        && !arbos_core::cloudsync::settled(place.path())
    {
        r.warn(
            ".arbos",
            None,
            arbos_core::cloudsync::advice(&sync, place.path()),
        );
    }

    // Worktrees of workers that are gone (archived or deleted): the kernel
    // removes a clean one when it archives the worker; a dirty one, or one
    // left by a kernel older than that, is named here.
    for id in crate::worktree::ids(place.path()) {
        if ids.contains(&id) {
            continue;
        }
        let Some(left) = crate::worktree::leftover(place.path(), &id) else {
            continue;
        };
        let what = if left.dirty == usize::MAX {
            "worktree of a worker that is gone; git could not read it".to_string()
        } else if left.dirty > 0 {
            format!(
                "worktree of a worker that is gone, with {} uncommitted path(s) on {}: commit or discard them, then `git worktree remove {}`",
                left.dirty,
                left.branch,
                left.path.display()
            )
        } else if left.ahead > 0 {
            format!(
                "worktree of a worker that is gone; clean, {} keeps {} commit(s): `git worktree remove {}` (the branch stays)",
                left.branch,
                left.ahead,
                left.path.display()
            )
        } else {
            format!(
                "worktree of a worker that is gone; clean and {} has nothing new: `git worktree remove {} && git branch -D {}`",
                left.branch,
                left.path.display(),
                left.branch
            )
        };
        r.warn(rel(&left.path), None, what);
    }

    // doors.toml: a chat door the kernel could not open is a warning; a
    // file it cannot read is an error (no door opens then).
    match crate::chatdoor::load(place) {
        Ok(doors) => {
            for d in doors {
                let source = arbos_engine::secrets::Config::load(place.path())
                    .ok()
                    .and_then(|c| c.secrets.get(d.token.trim()).cloned())
                    .unwrap_or_else(|| d.token.trim().to_string());
                let kind = arbos_engine::secrets::kind_of(&source);
                let reachable = if let Some(v) = source.strip_prefix("env:") {
                    std::env::var_os(v.trim()).is_some()
                } else if let Some(p) = source.strip_prefix("file:") {
                    Path::new(p.trim()).exists()
                } else if source.starts_with("op://") {
                    std::env::var_os("OP_SERVICE_ACCOUNT_TOKEN").is_some()
                        || std::env::var_os("OP_SESSION").is_some()
                } else {
                    false
                };
                if !reachable {
                    r.warn(
                        format!(".arbos/{}", crate::chatdoor::FILE),
                        None,
                        format!(
                            "{} door: token {:?} ({kind}) cannot be read from here; the kernel logs door_token and leaves the door closed",
                            d.kind, d.token
                        ),
                    );
                }
                if !arbos_core::agent_exists(place, &d.agent) {
                    r.warn(
                        format!(".arbos/{}", crate::chatdoor::FILE),
                        None,
                        format!("{} door: agent {:?} does not exist here", d.kind, d.agent),
                    );
                }
            }
        }
        Err(e) => r.error(
            format!(".arbos/{}", crate::chatdoor::FILE),
            None,
            format!("{e:#}"),
        ),
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
                if let Some(names) = v.get("stray_secret_env").and_then(|s| s.as_array())
                    && !names.is_empty()
                {
                    let list: Vec<&str> = names.iter().filter_map(|n| n.as_str()).collect();
                    r.warn(
                        rel(&kj),
                        None,
                        format!(
                            "the kernel's environment holds {} credential-looking variable(s) the secrets door does not manage ({}): they came from the shell it was started in; declare them in .arbos/secrets.toml or start it clean",
                            list.len(),
                            list.join(", ")
                        ),
                    );
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
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
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
                    r.error(
                        &file_rel,
                        None,
                        format!("id {} repeats another file's", sub.id),
                    );
                }
                ids.push(sub.id);
                if !name.starts_with(&format!("{:04}-", sub.id)) {
                    r.warn(
                        &file_rel,
                        None,
                        format!("file name does not start with {:04}-", sub.id),
                    );
                }
                if sub.next_due.is_some() && sub.next_due_ms().is_none() {
                    r.error(&file_rel, None, "next_due is not an RFC 3339 instant");
                }
            }
            Err(e) => r.error(&file_rel, None, format!("{e:#}")),
        }
    }
}

/// `waiting/*.toml` must parse; an `approve` mirror with no kernel running
/// is stale (the start clears it); an `ask` may stand for ever.
fn check_waiting(r: &mut Report, rel: &str, dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for p in rd.flatten().map(|e| e.path()) {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.starts_with('.') {
            continue;
        }
        let file_rel = format!("{rel}/{name}");
        match arbos_core::waiting::read(&p) {
            Ok(w) => {
                if !matches!(w.kind.as_str(), "ask" | "approve") {
                    r.error(
                        &file_rel,
                        None,
                        format!("kind {:?} is not ask or approve", w.kind),
                    );
                }
                if !name.starts_with(&format!("{}-", w.kind)) {
                    r.warn(
                        &file_rel,
                        None,
                        format!("file name does not start with {}-", w.kind),
                    );
                }
                if w.kind == "approve" {
                    r.warn(
                        &file_rel,
                        None,
                        "an approve mirror outlives no turn; the next kernel start removes it",
                    );
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
            r.warn(
                rel,
                Some(i + 1),
                format!(
                    "checkbox mark {:?} is not one the plan tool reads (space or x)",
                    &rest[..end]
                ),
            );
        }
    }
    let n = notes::read_path(path).items().len();
    if n > 200 {
        r.warn(
            rel,
            None,
            format!("{n} items; the prompt shows them all — archive the done ones"),
        );
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

/// Linux: every other process holding a file under `.arbos/` open for
/// writing, from /proc. Elsewhere: nothing (lsof is slow and not always
/// there). `(pid, command line, file)`.
fn writers_into(place: &Place) -> Vec<(u32, String, std::path::PathBuf)> {
    let mut out = Vec::new();
    let arbos = place.arbos();
    let arbos = std::fs::canonicalize(&arbos).unwrap_or(arbos);
    let me = std::process::id();
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return out;
    };
    for p in procs.flatten() {
        let Some(pid) = p.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        if pid == me {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(p.path().join("fd")) else {
            continue;
        };
        let mut seen_here = false;
        for fd in fds.flatten() {
            let Ok(target) = std::fs::read_link(fd.path()) else {
                continue;
            };
            if !target.starts_with(&arbos) {
                continue;
            }
            // fdinfo flags: octal; O_WRONLY = 1, O_RDWR = 2 in the low bits.
            let flags = std::fs::read_to_string(p.path().join("fdinfo").join(fd.file_name()))
                .ok()
                .and_then(|t| {
                    t.lines()
                        .find_map(|l| l.strip_prefix("flags:"))
                        .and_then(|f| u32::from_str_radix(f.trim(), 8).ok())
                })
                .unwrap_or(0);
            if flags & 0o3 == 0 || seen_here {
                continue;
            }
            seen_here = true;
            let args = std::fs::read(format!("/proc/{pid}/cmdline"))
                .map(|raw| {
                    String::from_utf8_lossy(&raw)
                        .replace('\0', " ")
                        .trim()
                        .to_string()
                })
                .unwrap_or_default();
            // The kernel itself (another kernel of this place) writes here
            // by design; a desktop's own files too.
            if args.contains("arbos-kernel") || args.contains("arbos-desktop") {
                continue;
            }
            out.push((pid, args, target));
        }
    }
    out
}
