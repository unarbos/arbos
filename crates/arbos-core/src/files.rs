use anyhow::{Context, Result, bail};
use std::{
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
};

use crate::{
    agent::{Agent, AgentId, Mode},
    event::{Event, EventKind},
    place::Place,
};

pub const ROOT_ID: &str = "root";

/// Paths under one agent folder.
pub struct Layout {
    pub dir: PathBuf,
}

impl Layout {
    pub fn new(place: &Place, id: &str) -> Self {
        Self {
            dir: place.agent_dir(id),
        }
    }

    pub fn agent_md(&self) -> PathBuf {
        self.dir.join("agent.md")
    }

    /// Human rendering of the plan. The kernel writes it.
    pub fn plan_md(&self) -> PathBuf {
        self.dir.join("plan.md")
    }

    /// The plan: one node per line, last line per id wins.
    pub fn plan_jsonl(&self) -> PathBuf {
        self.dir.join("plan.jsonl")
    }

    /// Every execution of a node, append-only.
    pub fn attempts_jsonl(&self) -> PathBuf {
        self.dir.join("attempts.jsonl")
    }

    pub fn transcript(&self) -> PathBuf {
        self.dir.join("transcript.jsonl")
    }

    /// Job folders: `jobs/jN/{meta.json,out.log,exit,...}`.
    pub fn jobs(&self) -> PathBuf {
        self.dir.join("jobs")
    }

    pub fn pages(&self) -> PathBuf {
        self.dir.join("pages")
    }

    /// Images a tool produced (screenshots). `read` on an existing image
    /// cites the file in place and does not copy it here.
    pub fn images(&self) -> PathBuf {
        self.dir.join("images")
    }

    pub fn wake_file(&self) -> PathBuf {
        self.dir.join("wake")
    }

    /// Standing instructions for this agent, shown in every instance
    /// prompt. Written at spawn from a definition's body; hand-editable.
    pub fn instructions(&self) -> PathBuf {
        self.dir.join("instructions.md")
    }
}

/// Create `.arbos/` and the root agent if they are missing.
/// What `.arbos/.gitignore` must hold: the process facts and the bulky
/// derived files. Everything else in `.arbos/` is the record and is
/// committed.
pub const ARBOS_GITIGNORE: &str =
    "# Process facts and caches: a running kernel, not the project. Never committed.
runtime/
# Where kernels before the runtime/ split kept the same facts.
kernel.json
lock
focus
checkpoint
kernel.log
web.json
index-scratch-*
agent.lock
kernel.stdout.log
kernel.out.log
# The Go-era session store and the desktop's own cache of chat records:
# large, churning, and derived from the transcripts that are committed.
sessions.db
sessions.db-*
desktop/
# Provider traces and job output: large, derived, and re-creatable.
agents/*/trace/
agents/*/jobs/*/out.log
agents/*/results/
# Children's checkouts are the project's own history, not this one's.
worktrees/
";

/// Make `.arbos/` its own git repository (nested, not a submodule: the
/// project ignores the folder) so every turn can be a commit and a rewind
/// is a checkout. Quiet when git is missing; the kernel runs without it.
/// Also keeps the project from ever tracking `.arbos/` by way of
/// `.git/info/exclude`, which is local and never committed.
pub fn init_arbos_repo(place: &Place) -> Result<bool> {
    let arbos = place.arbos();
    std::fs::create_dir_all(&arbos)?;
    let ignore = arbos.join(".gitignore");
    let have = std::fs::read_to_string(&ignore).unwrap_or_default();
    if have != ARBOS_GITIGNORE {
        // Keep a hand's extra lines; make sure ours are present.
        let mut merged = String::new();
        for line in ARBOS_GITIGNORE.lines() {
            if !have.lines().any(|l| l.trim() == line.trim()) {
                merged.push_str(line);
                merged.push('\n');
            }
        }
        let text = if have.trim().is_empty() {
            ARBOS_GITIGNORE.to_string()
        } else if merged.is_empty() {
            have.clone()
        } else {
            format!("{}\n{merged}", have.trim_end())
        };
        if text != have {
            std::fs::write(&ignore, text)?;
        }
    }
    // Every start, not only the first: a place that moved its store to
    // .arbos.nosync (cloudsync) needs that name excluded too, or a
    // `git add -A` in the project records the nested repository.
    exclude_locally(&place.path, &[".arbos/", ".arbos.nosync/"]);
    if place.arbos_repo().exists() {
        return Ok(false);
    }
    let init = std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&arbos)
        .stdin(std::process::Stdio::null())
        .output();
    let ok = match init {
        Ok(out) if out.status.success() => true,
        // An old git without `-b`: try again without it.
        Ok(_) => std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&arbos)
            .stdin(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false),
        Err(_) => false,
    };
    if !ok {
        return Ok(false);
    }
    Ok(true)
}

/// The project must never track the nested repository as a gitlink.
/// `.git/info/exclude` is the local, uncommitted ignore list: each of
/// `patterns` (a folder name with its slash) goes there unless git already
/// ignores it. Quiet when the project is not a repository.
pub fn exclude_locally(project: &Path, patterns: &[&str]) {
    let git_dir = project.join(".git");
    if !git_dir.exists() {
        return;
    }
    let exclude = git_dir.join("info").join("exclude");
    let _ = std::fs::create_dir_all(exclude.parent().unwrap());
    let mut text = std::fs::read_to_string(&exclude).unwrap_or_default();
    let mut changed = false;
    for pattern in patterns {
        let bare = pattern.trim_end_matches('/');
        let listed = text
            .lines()
            .any(|l| l.trim() == *pattern || l.trim() == bare);
        if listed {
            continue;
        }
        let ignored = std::process::Command::new("git")
            .args(["check-ignore", "-q", bare])
            .current_dir(project)
            .stdin(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ignored {
            continue;
        }
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(pattern);
        text.push('\n');
        changed = true;
    }
    if changed {
        let _ = std::fs::write(&exclude, text);
    }
}

pub fn bootstrap(place: &Place) -> Result<Agent> {
    std::fs::create_dir_all(place.agents_dir())?;
    std::fs::create_dir_all(place.archive_dir())?;
    std::fs::create_dir_all(place.runtime_dir())?;
    // Old kernels left these at the root; the record moves to runtime/.
    for name in ["focus", "checkpoint"] {
        let old = place.arbos().join(name);
        let new = place.runtime_dir().join(name);
        if old.exists() && !new.exists() {
            let _ = std::fs::rename(&old, &new);
        }
    }
    let _ = init_arbos_repo(place);
    if !place.user_md().exists() {
        std::fs::write(place.user_md(), "")?;
    }
    let _ = crate::store::ensure(place);
    crate::protocol::ensure(place);
    let root = Layout::new(place, ROOT_ID);
    if !root.agent_md().exists() {
        // A place that has never had a root is new: it starts in Cursor's
        // shape, with the main chat as a coordinator. An old place keeps
        // its behaviour until someone writes the line.
        let name = place
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("project");
        let _ = crate::project::write_for_new_place(place, name);
    }
    let agent = if root.agent_md().exists() {
        let mut agent = Agent::load(&root.dir)?;
        if agent.cwd.is_none() {
            agent.cwd = Some(place.path.clone());
            let _ = agent.save(&root.dir);
        }
        agent
    } else {
        let mut agent = Agent::root(ROOT_ID);
        agent.cwd = Some(place.path.clone());
        if let Some(mode) = mode_from_env() {
            agent.mode = mode;
        }
        agent.save(&root.dir)?;
        touch(&root.transcript())?;
        std::fs::create_dir_all(root.jobs())?;
        agent
    };
    // After root exists, so a missing or dangling focus can settle on it.
    let _ = read_focus(place);
    Ok(agent)
}

/// Env var naming the mode a freshly minted root agent starts in
/// (`auto`, `ask`, `plan`). A headless container sets `auto` so no call
/// waits on a question nobody will answer. An existing `agent.md` wins.
pub const MODE_ENV: &str = "ARBOS_MODE";

fn mode_from_env() -> Option<Mode> {
    let raw = std::env::var(MODE_ENV).ok()?;
    match Mode::parse(&raw) {
        Some(m) => Some(m),
        None => {
            eprintln!("{MODE_ENV}={raw:?} is not auto, ask, or plan; ignored");
            None
        }
    }
}

/// A new top-level chat: its own folder and empty transcript, copied
/// from `root`'s model and allowlist. Desktop `+` uses this so two
/// chats do not share `root`'s log.
pub fn create_chat(place: &Place) -> Result<Agent> {
    let template = bootstrap(place).ok();
    let mut n = crate::now_ms();
    let id = loop {
        let id = format!("chat-{n}");
        crate::validate_id(&id)?;
        if !place.agent_dir(&id).exists() {
            break id;
        }
        n += 1;
    };
    let mut agent = Agent::root(&id);
    agent.name = "chat".into();
    if let Some(t) = &template {
        agent.model = t.model.clone();
        agent.allowlist = t.allowlist.clone();
    }
    agent.cwd = Some(place.path.clone());
    agent.save(&place.agent_dir(&id))?;
    let layout = Layout::new(place, &id);
    touch(&layout.transcript())?;
    std::fs::create_dir_all(layout.jobs())?;
    Ok(agent)
}

/// A fork: a new chat carrying `source`'s transcript. Spawn records in the
/// copy are rewritten — their `child` link dropped and the body marked —
/// so the fork never claims the original's workers as its own (a window
/// listed them under the fork and, when the fork was itself one of them,
/// looped; #138). The original's folder is untouched.
pub fn fork_chat(place: &Place, source_id: &str) -> Result<Agent> {
    let source = Agent::load(&place.agent_dir(source_id))
        .with_context(|| format!("fork: no chat {source_id}"))?;
    let agent = create_chat(place)?;
    let id = agent.id.clone();
    // A fork that could not be finished is not left as a chat with half
    // a history and no mark: the folder goes with the error.
    fork_into(place, &source, agent).inspect_err(|_| {
        let _ = std::fs::remove_dir_all(place.agent_dir(id.as_str()));
    })
}

fn fork_into(place: &Place, source: &Agent, mut agent: Agent) -> Result<Agent> {
    let source_id = source.id.as_str();
    agent.model = source.model.clone();
    agent.allowlist = source.allowlist.clone();
    // A pinned mode is part of what the chat is for: the fork keeps it.
    agent.skill = source.skill.clone();
    agent.title = if source.title.is_empty() {
        String::new()
    } else {
        format!("{} (fork)", source.title)
    };
    agent.save(&place.agent_dir(agent.id.as_str()))?;
    let from = Layout::new(place, source_id).transcript();
    let to = Layout::new(place, agent.id.as_str()).transcript();
    if from.exists() {
        // Line for line: a blank or damaged line in the source stays a
        // line in the copy, so the checkpoints copied below (indexed by
        // line) still name the turns they were written for. Parsing and
        // re-appending dropped such lines and shifted every later
        // checkpoint onto the wrong turn.
        let raw = std::fs::read_to_string(&from)?;
        let mut out = String::with_capacity(raw.len());
        for line in raw.lines() {
            let rewritten = serde_json::from_str::<Event>(line).ok().and_then(|mut ev| {
                if let EventKind::Tool(rec) = &mut ev.kind
                    && rec.child.take().is_some()
                {
                    let note =
                        "(spawned by the chat this one was forked from; not this chat's worker)";
                    rec.body = Some(match rec.body.take() {
                        Some(b) => format!("{note}\n{b}"),
                        None => note.to_string(),
                    });
                    serde_json::to_string(&ev).ok()
                } else {
                    None
                }
            });
            out.push_str(rewritten.as_deref().unwrap_or(line));
            out.push('\n');
        }
        std::fs::write(&to, out)?;
    }
    // The checkpoints go with the transcript they index: line for line the
    // copy is the same file, so the fork's earlier turns stay rewindable.
    // Without them a rewind on a fork found no checkpoint before its own
    // first turn (F-103).
    let cps_from = Layout::new(place, source_id).dir.join("checkpoints.jsonl");
    if cps_from.exists() {
        let cps_to = Layout::new(place, agent.id.as_str())
            .dir
            .join("checkpoints.jsonl");
        std::fs::copy(&cps_from, &cps_to)?;
    }
    Ok(agent)
}

/// `child` on a spawn record, checked against disk: only when the named
/// agent exists and calls `agent` its parent. An old fork's copied record,
/// or a child since re-parented or removed, names nobody.
pub fn scrub_child_claims(place: &Place, agent: &str, ev: &mut Event) {
    if let EventKind::Tool(rec) = &mut ev.kind
        && let Some(child) = rec.child.as_deref()
    {
        let ok = child != agent
            && Agent::load(&place.agent_dir(child))
                .ok()
                .is_some_and(|c| c.parent.as_ref().is_some_and(|p| p.as_str() == agent));
        if !ok {
            rec.child = None;
        }
    }
}

/// The focus names one agent folder of this place, as `.arbos/agents/<id>`.
/// Accepts that form or a bare `<id>`. Anything else is refused: the file
/// is written by whoever can reach the attach socket and read back by every
/// client and every prompt, so it must never carry an arbitrary path
/// (QA bug qa-004).
pub fn validate_focus(place: &Place, path: &str) -> Result<String> {
    let trimmed = path.trim().trim_start_matches("./");
    let id = trimmed
        .strip_prefix(".arbos/agents/")
        .unwrap_or(trimmed)
        .trim_end_matches('/');
    crate::validate_id(id).with_context(|| format!("focus {path:?}"))?;
    if !place.agent_dir(id).join("agent.md").exists() {
        bail!("focus {path:?}: no agent {id}");
    }
    Ok(format!(".arbos/agents/{id}"))
}

fn root_focus() -> String {
    format!(".arbos/agents/{ROOT_ID}")
}

/// The focused agent folder. A missing or invalid file reads as root, and
/// is rewritten so every reader agrees.
pub fn read_focus(place: &Place) -> String {
    let raw = std::fs::read_to_string(place.focus_path()).unwrap_or_default();
    match validate_focus(place, &raw) {
        Ok(focus) => focus,
        Err(_) => {
            let _ = std::fs::write(place.focus_path(), format!("{}\n", root_focus()));
            root_focus()
        }
    }
}

pub fn write_focus(place: &Place, path: &str) -> Result<()> {
    let focus = validate_focus(place, path)?;
    std::fs::write(place.focus_path(), format!("{focus}\n"))
        .with_context(|| format!("write {}", place.focus_path().display()))
}

/// Append one event and fsync.
pub fn append_event(path: &Path, event: &Event) -> Result<usize> {
    append_events(path, std::slice::from_ref(event))
}

/// Several lines, one write, one fsync. The running turn is not the only
/// writer: `say` from another agent, an answer, a notice from the kernel
/// all append to the same file while a turn runs. One `write` call with
/// `O_APPEND` lands whole; separate calls could interleave and leave a
/// line that no reader can parse. Do not reread the file to count lines —
/// that was on the path to the first token.
pub fn append_events(path: &Path, events: &[Event]) -> Result<usize> {
    if events.is_empty() {
        return Ok(0);
    }
    // The folder is made once, by bootstrap/create_chat/spawn. An append
    // never recreates it: a chat deleted with `rm -rf` while a turn ran
    // came back as a ghost (a transcript with no agent.md) on the model's
    // late reply (qa-017). A missing folder is the end of that transcript.
    if let Some(parent) = path.parent()
        && !parent.is_dir()
    {
        bail!(
            "agent folder is gone; nothing more is written to {}",
            path.display()
        );
    }
    // One batch, one millisecond: a queued prompt's `wake` and `user`
    // lines carried the same `ts`, and a client that orders by time (the
    // phone, a replay) could not tell which came first. Within a batch
    // each line's `ts` is at least one past the line before it; the
    // order on disk and the order in time then agree.
    let mut buf = Vec::with_capacity(events.len() * 256);
    let mut last_ts: Option<i64> = None;
    for event in events {
        let mut owned;
        let event = match last_ts {
            Some(prev) if event.ts <= prev => {
                owned = event.clone();
                owned.ts = prev + 1;
                &owned
            }
            _ => event,
        };
        last_ts = Some(event.ts);
        serde_json::to_writer(&mut buf, event)?;
        buf.push(b'\n');
    }
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    if let Err(e) = file.write_all(&buf) {
        drop_partial_line(&mut file, buf.len());
        return Err(e).with_context(|| format!("append {}", path.display()));
    }
    file.sync_data()?;
    Ok(events.len())
}

/// A write that failed part-way (disk full, size limit) leaves the head of
/// a line with no newline. Every reader skips it, but it also swallows the
/// next successful append into one damaged line. Cut the file back to the
/// last complete line. `wrote_at_most` bounds how far back to look.
pub fn drop_partial_line(file: &mut File, wrote_at_most: usize) {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(len) = file.metadata().map(|m| m.len()) else {
        return;
    };
    let look = (wrote_at_most as u64 + 1).min(len);
    if look == 0 || file.seek(SeekFrom::Start(len - look)).is_err() {
        return;
    }
    let mut tail = vec![0u8; look as usize];
    if file.read_exact(&mut tail).is_err() {
        return;
    }
    if tail.last() == Some(&b'\n') {
        return;
    }
    let keep = match tail.iter().rposition(|b| *b == b'\n') {
        Some(i) => len - look + i as u64 + 1,
        None => len - look,
    };
    let _ = file.set_len(keep);
}

/// Every parseable event, each stamped with its 1-based physical line.
/// Blank and unparseable lines are skipped but still counted, so `seq`
/// matches `grep -n` and never shifts when a line is damaged.
pub fn load_transcript(path: &Path) -> Result<Vec<Event>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = File::open(path)?;
    let mut out = Vec::new();
    for (i, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(mut ev) = serde_json::from_str::<Event>(&line) {
            ev.seq = i as u64 + 1;
            out.push(ev);
        }
    }
    Ok(out)
}

/// What one transcript roll did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rolled {
    pub archive: PathBuf,
    pub lines: u64,
}

/// Phase 5c, first step. A standing agent's transcript grows without end
/// (17,969 lines on the Mac). Past `max_lines`, at a turn end, the file
/// moves to `transcript-archive/NNNN.jsonl` and a fresh transcript opens
/// with one `compaction` line that carries the newest compaction summary
/// forward (or a pointer to the archive when there is none), so the model
/// keeps what it knew and every reader sees a short file. Nothing is
/// deleted; `grep scope=history` and `read` still reach the archive.
pub fn roll_transcript(place: &Place, agent: &str, max_lines: u64) -> Result<Option<Rolled>> {
    if max_lines == 0 {
        return Ok(None);
    }
    let layout = Layout::new(place, agent);
    let path = layout.transcript();
    let events = load_transcript(&path)?;
    let lines = events.len() as u64;
    if lines <= max_lines {
        return Ok(None);
    }
    // Only between turns: the last line is a turn's end.
    if !matches!(
        events.last().map(|e| &e.kind),
        Some(EventKind::TurnComplete { .. })
    ) {
        return Ok(None);
    }
    let dir = layout.dir.join("transcript-archive");
    std::fs::create_dir_all(&dir)?;
    let n = std::fs::read_dir(&dir)?
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()?
                .strip_suffix(".jsonl")?
                .parse::<u32>()
                .ok()
        })
        .max()
        .unwrap_or(0)
        + 1;
    let archive = dir.join(format!("{n:04}.jsonl"));
    let carried = events.iter().rev().find_map(|e| match &e.kind {
        EventKind::Compaction { summary, .. } => Some(summary.clone()),
        _ => None,
    });
    let rel = format!(".arbos/agents/{agent}/transcript-archive/{n:04}.jsonl");
    let summary = match carried {
        Some(s) => format!(
            "{s}\n\n[history rolled: the {lines} earlier lines are in {rel}; grep path=.arbos/agents/{agent} or read it for detail]"
        ),
        None => format!(
            "[history rolled: the {lines} earlier lines of this transcript are in {rel}; grep path=.arbos/agents/{agent} or read it for detail]"
        ),
    };
    std::fs::rename(&path, &archive)?;
    // The checkpoints index the file by line, so they roll with it.
    // Left behind, they indexed the *old* file: after a roll `rewind
    // turn 1` resolved to the project's very first checkpoint (its line
    // was small enough) and a files rewind would have put the working
    // tree back months — the "cut too much" shape, a third time. Into the
    // archive beside the lines they describe; a rewind into rolled
    // history is refused rather than guessed.
    for (from, to) in [
        (
            layout.dir.join("checkpoints.jsonl"),
            dir.join(format!("{n:04}.checkpoints.jsonl")),
        ),
        (
            layout.dir.join("checkpoints.d"),
            dir.join(format!("{n:04}.checkpoints.d")),
        ),
    ] {
        if from.exists() {
            std::fs::rename(&from, &to)?;
        }
    }
    let opener = Event::new(EventKind::Compaction {
        lo: 1,
        hi: lines,
        summary,
        tokens_before: 0,
        tokens_after: 0,
        model: String::new(),
    });
    append_event(&path, &opener)?;
    Ok(Some(Rolled { archive, lines }))
}

/// Incremental reader for one append-only transcript. Remembers how far it
/// has read, so a poll costs the new bytes, not a re-parse of the file.
/// The serve loop polls every agent five times a second; re-parsing a
/// multi-megabyte transcript each time starved the loop until Ctrl-C was
/// ignored (QA bug qa-003).
#[derive(Debug, Default, Clone)]
pub struct TranscriptTail {
    /// Bytes consumed: always the position just after a newline.
    pub offset: u64,
    /// Physical lines consumed, blank and damaged ones included, so `seq`
    /// matches `load_transcript`.
    pub lines: u64,
    /// Identity of the file the offset belongs to. A different inode is a
    /// different file (the folder was deleted and recreated, or a fork's
    /// transcript was copied in), whatever its length.
    identity: Option<(u64, u64)>,
}

fn file_identity(meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some((meta.dev(), meta.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        None
    }
}

impl TranscriptTail {
    /// Events on lines appended since the last call, each stamped with its
    /// 1-based physical line. A file that shrank below the offset, or that
    /// is a different file than last time, was replaced; the tail starts
    /// over from its beginning.
    pub fn read_new(&mut self, path: &Path) -> Result<Vec<Event>> {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut file) = File::open(path) else {
            return Ok(Vec::new());
        };
        let meta = file.metadata()?;
        let len = meta.len();
        let identity = file_identity(&meta);
        let replaced = len < self.offset || (self.identity.is_some() && identity != self.identity);
        if replaced {
            *self = Self::default();
        }
        self.identity = identity;
        if len == self.offset {
            return Ok(Vec::new());
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut buf = Vec::with_capacity((len - self.offset) as usize);
        file.take(len - self.offset).read_to_end(&mut buf)?;
        let mut out = Vec::new();
        let mut start = 0usize;
        while let Some(rel) = buf[start..].iter().position(|b| *b == b'\n') {
            let line = &buf[start..start + rel];
            start += rel + 1;
            self.lines += 1;
            self.offset += (rel + 1) as u64;
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            if let Ok(mut ev) = serde_json::from_slice::<Event>(line) {
                ev.seq = self.lines;
                out.push(ev);
            }
        }
        // A trailing partial line (a writer mid-append) waits for its newline.
        Ok(out)
    }
}

pub fn list_agents(place: &Place) -> Result<Vec<Agent>> {
    let dir = place.agents_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut agents = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(&dir)?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        if let Ok(agent) = Agent::load(&entry.path()) {
            agents.push(agent);
        }
    }
    Ok(agents)
}

/// Folders under `agents/` that `list_agents` leaves out, with the reason
/// (`agents/<name>: no agent.md`, `…: agent.md: <parse error>`). The
/// kernel logs them at boot so a folder that silently vanished from the
/// roster (ba2262db79) is named somewhere a person looks; `check` reports
/// the same set. Root's folder before bootstrap is not one of them.
pub fn unlisted_agent_dirs(place: &Place) -> Vec<String> {
    let dir = place.agents_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    let mut out = Vec::new();
    for entry in entries {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let md = path.join("agent.md");
        if !md.exists() {
            if name != ROOT_ID {
                out.push(format!("agents/{name}: no agent.md"));
            }
            continue;
        }
        if let Err(e) = Agent::load(&path) {
            out.push(format!("agents/{name}: agent.md: {e:#}"));
        }
    }
    out
}

pub fn load_agent(place: &Place, id: &AgentId) -> Result<Agent> {
    Agent::load(&place.agent_dir(id.as_str()))
}

/// The one rule for "this id is an agent here": the same one `list_agents`
/// applies (a folder with a parseable `agent.md`). Every path that writes
/// into an agent folder checks this first, so a prompt to a folder the
/// kernel does not list cannot be stored and never fire (qa-006), and an
/// answer for an unknown id cannot mint a ghost folder (qa-005).
pub fn agent_exists(place: &Place, id: &str) -> bool {
    crate::validate_id(id).is_ok() && Agent::load(&place.agent_dir(id)).is_ok()
}

/// The transcript a client's `history` or attach replays for `id`: the
/// live agent's, or — once the agent has finished and its folder moved
/// under `archive/agents/` — the archived one, flagged. A Done worker's
/// chat read "Nothing on record yet" while its whole record sat in the
/// archive (M-27). None when no folder of that name exists in either.
pub fn transcript_for_history(place: &Place, id: &str) -> Option<(std::path::PathBuf, bool)> {
    resolve_history_agent(place, id).map(|r| (r.transcript, r.archived))
}

/// What a `history` request resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryTarget {
    /// The folder id, live or archived.
    pub id: String,
    pub transcript: std::path::PathBuf,
    pub archived: bool,
}

/// The agent a client means by `q`: its id, live or archived — or its
/// *name* (the spawn's `name`, what the roster and a worker card show),
/// live first, then archived. The phone asked `history` for "Run J152618
/// verification command", the name on its sheet, and got `total: 0` for
/// eight of eight finished workers whose records were on disk under
/// their ids; a name is not a valid id, so nothing was even looked up.
pub fn resolve_history_agent(place: &Place, q: &str) -> Option<HistoryTarget> {
    let q = q.trim();
    let by_id = |id: &str| -> Option<HistoryTarget> {
        if crate::validate_id(id).is_err() {
            return None;
        }
        if place.agent_dir(id).join("agent.md").exists() {
            return Some(HistoryTarget {
                id: id.to_string(),
                transcript: Layout::new(place, id).transcript(),
                archived: false,
            });
        }
        let dir = crate::project::archive_agents_dir(place).join(id);
        dir.join("agent.md").exists().then(|| HistoryTarget {
            id: id.to_string(),
            transcript: dir.join("transcript.jsonl"),
            archived: true,
        })
    };
    if let Some(t) = by_id(q) {
        return Some(t);
    }
    // By name, case-folded: the live roster, then the archive.
    let want = q.to_lowercase();
    if let Ok(agents) = list_agents(place)
        && let Some(a) = agents.iter().find(|a| a.name.trim().to_lowercase() == want)
    {
        return by_id(a.id.as_str());
    }
    let rd = std::fs::read_dir(crate::project::archive_agents_dir(place)).ok()?;
    let mut hits: Vec<(std::path::PathBuf, Agent)> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter_map(|p| Agent::load(&p).ok().map(|a| (p, a)))
        .filter(|(_, a)| a.name.trim().to_lowercase() == want)
        .collect();
    // Two archived workers with one name: the newest folder (by mtime).
    hits.sort_by_key(|(p, _)| std::fs::metadata(p).and_then(|m| m.modified()).ok());
    let (dir, a) = hits.pop()?;
    Some(HistoryTarget {
        id: a.id.to_string(),
        transcript: dir.join("transcript.jsonl"),
        archived: true,
    })
}

/// `id`, its parent, grandparent, … up to the top (or an unreadable or
/// looping link). What a scoped grant is checked against.
pub fn lineage(place: &Place, id: &str) -> Vec<String> {
    let mut out = vec![id.to_string()];
    let mut cur = id.to_string();
    while let Ok(a) = Agent::load(&place.agent_dir(&cur)) {
        let Some(p) = a.parent else {
            break;
        };
        let p = p.to_string();
        if out.contains(&p) || out.len() > 64 {
            break;
        }
        out.push(p.clone());
        cur = p;
    }
    out
}

/// `id` and every agent under it, by the parent links on disk.
pub fn subtree(place: &Place, id: &str) -> Vec<String> {
    let agents = list_agents(place).unwrap_or_default();
    let mut out = vec![id.to_string()];
    let mut i = 0;
    while i < out.len() {
        for a in &agents {
            if a.parent.as_ref().is_some_and(|p| p.as_str() == out[i])
                && !out.iter().any(|x| x == a.id.as_str())
            {
                out.push(a.id.to_string());
            }
        }
        i += 1;
    }
    out
}

fn touch(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    File::create(path)?;
    Ok(())
}

/// Incomplete wake: last wake has no following `turn_complete`, or a `wake` file exists.
pub fn needs_serve(place: &Place, id: &str) -> bool {
    let layout = Layout::new(place, id);
    if layout.wake_file().exists() {
        return true;
    }
    let Ok(events) = load_transcript(&layout.transcript()) else {
        return false;
    };
    let mut last_wake = None;
    let mut last_complete = None;
    for (i, ev) in events.iter().enumerate() {
        if ev.is_wake() {
            last_wake = Some(i);
        }
        if ev.is_turn_complete() {
            last_complete = Some(i);
        }
    }
    match (last_wake, last_complete) {
        (Some(w), Some(c)) => c < w,
        (Some(_), None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod roll_tests {
    use super::*;
    use crate::event::EventKind;

    /// A fork copies the transcript line for line, a damaged line
    /// included, so the checkpoints copied with it (indexed by line)
    /// still name the turns they were written for; and a fork that
    /// cannot be finished leaves no half chat behind.
    #[test]
    fn a_fork_keeps_line_numbers_across_a_damaged_line_and_leaves_no_half_chat_on_error() {
        let dir = tempfile::tempdir().unwrap();
        let place = Place::new(dir.path());
        std::fs::create_dir_all(place.arbos()).unwrap();
        let root = create_chat(&place).unwrap();
        let layout = Layout::new(&place, root.id.as_str());
        let path = layout.transcript();
        let user = |t: &str| {
            Event::new(EventKind::User {
                text: t.into(),
                attachments: vec![],
                channel: String::new(),
                device: String::new(),
            })
        };
        append_event(&path, &user("one")).unwrap();
        // A damaged line, as a crash mid-append leaves one.
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"ts\":1,\"kind\":\"assis\n")
            .unwrap();
        append_event(&path, &user("two")).unwrap();
        // A checkpoint at the second user's line, line 3 (0-based 2).
        std::fs::write(
            layout.dir.join("checkpoints.jsonl"),
            "{\"line\":2,\"ts\":0,\"head\":\"h2\"}\n",
        )
        .unwrap();
        let fork = fork_chat(&place, root.id.as_str()).unwrap();
        let copy = Layout::new(&place, fork.id.as_str()).transcript();
        let raw = std::fs::read_to_string(&copy).unwrap();
        assert_eq!(
            raw.lines().count(),
            3,
            "three lines in, three lines out:\n{raw}"
        );
        assert!(
            raw.lines()
                .nth(1)
                .unwrap()
                .starts_with("{\"ts\":1,\"kind\":\"assis")
        );
        let events = load_transcript(&copy).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].seq, 3, "the second user line is still line 3");
        assert!(
            Layout::new(&place, fork.id.as_str())
                .dir
                .join("checkpoints.jsonl")
                .exists()
        );

        // A source whose transcript cannot be read (a directory in its
        // place) is an error after the fork's folder was made — and the
        // folder does not stay behind as a chat with no history.
        let before: Vec<_> = list_agents(&place)
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(fork_chat(&place, root.id.as_str()).is_err());
        let after: Vec<_> = list_agents(&place)
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(after, before, "no half fork left behind");
    }

    #[test]
    fn a_long_transcript_rolls_into_the_archive_and_keeps_the_last_summary() {
        let dir = tempfile::tempdir().unwrap();
        let place = Place::new(dir.path());
        let layout = Layout::new(&place, "root");
        std::fs::create_dir_all(&layout.dir).unwrap();
        let path = layout.transcript();
        for i in 0..30 {
            if i == 10 {
                append_event(
                    &path,
                    &Event::new(EventKind::Compaction {
                        lo: 1,
                        hi: 9,
                        summary: "we decided on blue".into(),
                        tokens_before: 0,
                        tokens_after: 0,
                        model: String::new(),
                    }),
                )
                .unwrap();
            }
            append_event(
                &path,
                &Event::new(EventKind::Assistant {
                    text: format!("line {i}"),
                    step: 0,
                    reasoning_details: None,
                }),
            )
            .unwrap();
        }
        // Not between turns: nothing happens.
        assert!(roll_transcript(&place, "root", 20).unwrap().is_none());
        append_event(&path, &Event::new(EventKind::TurnComplete { usage: None })).unwrap();
        // Under the cap: nothing happens.
        assert!(roll_transcript(&place, "root", 100).unwrap().is_none());
        // Checkpoints index the file by line: they roll with it.
        let agent_dir = place.agent_dir("root");
        std::fs::write(
            agent_dir.join("checkpoints.jsonl"),
            "{\"line\":1,\"ts\":0,\"head\":\"a\"}\n{\"line\":20,\"ts\":0,\"head\":\"b\"}\n",
        )
        .unwrap();
        std::fs::create_dir_all(agent_dir.join("checkpoints.d")).unwrap();
        std::fs::write(agent_dir.join("checkpoints.d/20.json"), "{}").unwrap();
        let rolled = roll_transcript(&place, "root", 20).unwrap().unwrap();
        assert_eq!(rolled.lines, 32);
        assert!(rolled.archive.ends_with("transcript-archive/0001.jsonl"));
        assert!(
            !agent_dir.join("checkpoints.jsonl").exists(),
            "the old file's checkpoints do not describe the new file"
        );
        assert!(!agent_dir.join("checkpoints.d").exists());
        assert!(
            agent_dir
                .join("transcript-archive/0001.checkpoints.jsonl")
                .exists()
        );
        assert!(
            agent_dir
                .join("transcript-archive/0001.checkpoints.d/20.json")
                .exists()
        );
        let archived = load_transcript(&rolled.archive).unwrap();
        assert_eq!(archived.len(), 32);
        let fresh = load_transcript(&path).unwrap();
        assert_eq!(fresh.len(), 1);
        match &fresh[0].kind {
            EventKind::Compaction { summary, hi, .. } => {
                assert!(summary.starts_with("we decided on blue"), "{summary}");
                assert!(
                    summary.contains("transcript-archive/0001.jsonl"),
                    "{summary}"
                );
                assert_eq!(*hi, 32);
            }
            other => panic!("{other:?}"),
        }
        // Off: never.
        assert!(roll_transcript(&place, "root", 0).unwrap().is_none());
    }
}

#[cfg(test)]
mod batch_ts_tests {
    use super::*;
    use crate::event::EventKind;

    /// Symmetry loop: a queued prompt's `wake` and `user` lines carried the
    /// same `ts`, so a client that orders by time could not tell which
    /// came first. Within one batch, each line's `ts` is past the one
    /// before it; a batch already in order is written as it is.
    #[test]
    fn lines_of_one_batch_have_strictly_increasing_timestamps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transcript.jsonl");
        let t = 1_789_500_000_000i64;
        let mut wake = Event::new(EventKind::Wake {
            wake: "user".into(),
            text: Some("hi".into()),
            brief: None,
        });
        wake.ts = t;
        let mut user = Event::new(EventKind::User {
            text: "hi".into(),
            attachments: vec![],
            channel: String::new(),
            device: String::new(),
        });
        user.ts = t;
        let mut later = Event::new(EventKind::Assistant {
            text: "hello".into(),
            step: 1,
            reasoning_details: None,
        });
        later.ts = t + 50;
        append_events(&path, &[wake, user, later]).unwrap();
        let back = load_transcript(&path).unwrap();
        assert_eq!(back.len(), 3);
        assert_eq!(back[0].ts, t);
        assert_eq!(back[1].ts, t + 1, "the second line is one past the first");
        assert_eq!(
            back[2].ts,
            t + 50,
            "a line already later keeps its own time"
        );
        assert!(back.windows(2).all(|w| w[0].ts < w[1].ts));
    }
}
