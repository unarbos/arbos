use anyhow::{Context, Result, bail};
use std::{
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
};

use crate::{
    agent::{Agent, AgentId, Mode},
    event::Event,
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
pub fn bootstrap(place: &Place) -> Result<Agent> {
    std::fs::create_dir_all(place.agents_dir())?;
    std::fs::create_dir_all(place.archive_dir())?;
    if !place.user_md().exists() {
        std::fs::write(place.user_md(), "")?;
    }
    let root = Layout::new(place, ROOT_ID);
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
        if !root.plan_md().exists() {
            std::fs::write(root.plan_md(), "")?;
        }
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
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut buf = Vec::with_capacity(events.len() * 256);
    for event in events {
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
