use anyhow::{Context, Result};
use std::{
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
};

use crate::{
    agent::{Agent, AgentId},
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
}

/// Create `.arbos/` and the root agent if they are missing.
pub fn bootstrap(place: &Place) -> Result<Agent> {
    std::fs::create_dir_all(place.agents_dir())?;
    std::fs::create_dir_all(place.archive_dir())?;
    if !place.user_md().exists() {
        std::fs::write(place.user_md(), "")?;
    }
    if !place.focus_path().exists() {
        std::fs::write(place.focus_path(), format!(".arbos/agents/{ROOT_ID}\n"))?;
    }
    let root = Layout::new(place, ROOT_ID);
    if root.agent_md().exists() {
        let mut agent = Agent::load(&root.dir)?;
        if agent.cwd.is_none() {
            agent.cwd = Some(place.path.clone());
            let _ = agent.save(&root.dir);
        }
        return Ok(agent);
    }
    let mut agent = Agent::root(ROOT_ID);
    agent.cwd = Some(place.path.clone());
    agent.save(&root.dir)?;
    touch(&root.transcript())?;
    std::fs::create_dir_all(root.jobs())?;
    if !root.plan_md().exists() {
        std::fs::write(root.plan_md(), "")?;
    }
    Ok(agent)
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

pub fn read_focus(place: &Place) -> String {
    std::fs::read_to_string(place.focus_path())
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub fn write_focus(place: &Place, path: &str) -> Result<()> {
    std::fs::write(place.focus_path(), format!("{}\n", path.trim()))
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
        .append(true)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    file.write_all(&buf)?;
    file.sync_data()?;
    Ok(events.len())
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
