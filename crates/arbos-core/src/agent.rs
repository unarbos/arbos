use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// Folder name under `.arbos/agents/`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct AgentId(pub String);

impl AgentId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Tools the kernel ships. An allowlist is a subset of this list.
pub const ALL_TOOLS: &[&str] = &[
    "ls",
    "read",
    "find",
    "grep",
    "write",
    "edit",
    "apply_patch",
    "bash",
    "await",
    "jobs",
    "fetch",
    "search",
    "spawn",
    "say",
    "ask",
    "plan",
    "changes",
    "undo",
    "browser",
    "terminal",
    "screenshot",
    "secret",
    "subscribe",
];

/// How much an agent may do without asking. `agent.md` `mode:`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Writes run; only dangerous shell commands ask. The default.
    #[default]
    Auto,
    /// Every call that writes asks the user first.
    Ask,
    /// No writes at all: read, think, plan, report.
    Plan,
}

impl Mode {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Ask, Self::Plan];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Ask => "ask",
            Self::Plan => "plan",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "auto" | "yolo" | "full" => Some(Self::Auto),
            "ask" | "approve" | "confirm" => Some(Self::Ask),
            "plan" | "readonly" | "read-only" => Some(Self::Plan),
            _ => None,
        }
    }

    /// The picker's label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Ask => "Ask before writes",
            Self::Plan => "Plan only",
        }
    }

    /// One line for the prompt.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Auto => "auto: your edits and commands run; only dangerous shell commands ask.",
            Self::Ask => {
                "ask: every call that writes (write, edit, apply_patch, a bash command that is not read-only, undo, an MCP tool) shows the user an allow/deny question before it runs; reads run freely. Just make the call — the question is asked for you; do not ask permission in words first."
            }
            Self::Plan => {
                "plan: you may not write. Read, think, and put the change you would make in your plan and your reply; ask the user to switch you to ask or auto to carry it out."
            }
        }
    }
}

/// One agent folder. Fields live in `agent.md`.
#[derive(Debug, Clone)]
pub struct Agent {
    pub id: AgentId,
    pub name: String,
    /// Sidebar label from the first prompt. Empty until one is set.
    /// Distinct from [`Self::name`], which is the agent identity.
    pub title: String,
    pub parent: Option<AgentId>,
    pub paused: bool,
    pub model: String,
    pub allowlist: Vec<String>,
    pub readonly: bool,
    pub cwd: Option<PathBuf>,
    /// `machine:path` when this agent is a stand-in for a kernel on another
    /// machine: its turns run there; this folder mirrors them.
    pub remote: Option<String>,
    pub mode: Mode,
    /// The definition this agent was spawned from (`spawn kind=<name>`),
    /// or empty. Its body lives in the agent folder as `instructions.md`.
    pub kind: String,
}

impl Agent {
    pub fn root(id: impl Into<String>) -> Self {
        Self {
            id: AgentId::new(id),
            name: "root".into(),
            title: String::new(),
            parent: None,
            paused: false,
            model: "inherit".into(),
            allowlist: ALL_TOOLS.iter().map(|s| (*s).to_string()).collect(),
            readonly: false,
            cwd: None,
            remote: None,
            mode: Mode::Auto,
            kind: String::new(),
        }
    }

    pub fn dir(&self, place_agents: &Path) -> PathBuf {
        place_agents.join(self.id.as_str())
    }

    pub fn depth(&self, agents: &[Agent]) -> usize {
        let mut depth = 0;
        let mut cur = self.parent.clone();
        while let Some(pid) = cur {
            depth += 1;
            cur = agents
                .iter()
                .find(|a| a.id == pid)
                .and_then(|a| a.parent.clone());
            if depth > 32 {
                break;
            }
        }
        depth
    }

    pub fn to_md(&self) -> String {
        let parent = self
            .parent
            .as_ref()
            .map(|p| p.as_str().to_string())
            .unwrap_or_default();
        let cwd = self
            .cwd
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let remote = self.remote.clone().unwrap_or_default();
        format!(
            "name: {}\ntitle: {}\nparent: {}\npaused: {}\nmodel: {}\nallowlist: {}\nreadonly: {}\ncwd: {}\nremote: {}\nmode: {}\nkind: {}\n",
            self.name,
            self.title,
            parent,
            self.paused,
            self.model,
            self.allowlist.join(", "),
            self.readonly,
            cwd,
            remote,
            self.mode.as_str(),
            self.kind
        )
    }

    pub fn parse(id: AgentId, text: &str) -> Result<Self> {
        let mut agent = Agent::root(id.0.clone());
        agent.id = id;
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();
            match key {
                "name" if !value.is_empty() => agent.name = value.to_string(),
                "title" => agent.title = value.to_string(),
                "parent" => {
                    agent.parent = if value.is_empty() {
                        None
                    } else {
                        Some(AgentId::new(value))
                    }
                }
                "paused" => agent.paused = parse_bool(value),
                "model" if !value.is_empty() => agent.model = value.to_string(),
                "allowlist" => {
                    if !value.is_empty() {
                        agent.allowlist = value
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                    }
                }
                "readonly" => agent.readonly = parse_bool(value),
                "mode" => match Mode::parse(value) {
                    Some(m) => agent.mode = m,
                    None => eprintln!("{}: unknown mode {value:?}; using auto", agent.id),
                },
                "cwd" => {
                    agent.cwd = if value.is_empty() {
                        None
                    } else {
                        Some(PathBuf::from(value))
                    }
                }
                "remote" => {
                    agent.remote = (!value.is_empty()).then(|| value.to_string());
                }
                "kind" => agent.kind = value.to_string(),
                _ => {}
            }
        }
        // Agents minted before this tool still have bash. Grant the visible
        // shell the same way apply_patch follows edit.
        if !agent.allowlist.iter().any(|t| t == "terminal")
            && agent.allowlist.iter().any(|t| t == "bash")
        {
            agent.allowlist.push("terminal".into());
        }
        if !agent.allowlist.iter().any(|t| t == "screenshot")
            && agent.allowlist.iter().any(|t| t == "browser")
        {
            agent.allowlist.push("screenshot".into());
        }
        Ok(agent)
    }

    pub fn load(dir: &Path) -> Result<Self> {
        let id = dir
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow::anyhow!("agent dir has no name"))?;
        let text = std::fs::read_to_string(dir.join("agent.md"))
            .with_context(|| format!("read {}", dir.join("agent.md").display()))?;
        Self::parse(AgentId::new(id), &text)
    }

    /// Write `agent.md` atomically. The kernel and the desktop both save
    /// this file; a reader must never see it truncated (an empty file parses
    /// as an agent with every default: unpaused, full allowlist).
    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::create_dir_all(dir.join("pages"))?;
        let path = dir.join("agent.md");
        let tmp = dir.join(format!(".agent.md.{}.tmp", std::process::id()));
        std::fs::write(&tmp, self.to_md()).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("replace {}", path.display()))?;
        Ok(())
    }

    pub fn restrict_allowlist(&mut self, parent: &Agent) {
        self.allowlist
            .retain(|t| parent.allowlist.iter().any(|p| p == t));
        // A child is never freer than its parent.
        if parent.mode == Mode::Plan {
            self.mode = Mode::Plan;
        } else if parent.mode == Mode::Ask && self.mode == Mode::Auto {
            self.mode = Mode::Ask;
        }
        if self.readonly {
            self.allowlist.retain(|t| {
                matches!(
                    t.as_str(),
                    "ls" | "read"
                        | "find"
                        | "grep"
                        | "fetch"
                        | "search"
                        | "await"
                        | "jobs"
                        | "ask"
                        | "plan"
                        | "say"
                        | "screenshot"
                        | "secret"
                        | "subscribe"
                )
            });
        }
    }

    /// Plan mode is readonly by another name.
    pub fn no_writes(&self) -> bool {
        self.readonly || self.mode == Mode::Plan
    }

    pub fn may(&self, tool: &str) -> bool {
        // The plan is the agent's own intent. Every agent has one; old
        // agent.md files predate the tool.
        if tool == "plan" {
            return true;
        }
        if self.allowlist.iter().any(|t| t == tool) {
            return true;
        }
        // MCP tools are configured by the operator (ARBOS_MCP_CMD); every
        // agent may call them. `mcp__<name>` is the kernel's prefix.
        if tool.starts_with("mcp__") {
            return true;
        }
        // Same write surface as edit. Old agent.md files list edit only.
        if tool == "apply_patch" && self.allowlist.iter().any(|t| t == "edit") {
            return true;
        }
        // Keys into the shell's environment: goes with the shell. Old
        // agent.md files predate the tool.
        if tool == "secret" && self.allowlist.iter().any(|t| t == "bash") {
            return true;
        }
        // Following a pull request is standing work, like a plan node.
        // Old agent.md files predate the tool.
        if tool == "subscribe" && self.allowlist.iter().any(|t| t == "plan") {
            return true;
        }
        // Visible shell. Old agent.md files list bash only.
        if tool == "terminal" && self.allowlist.iter().any(|t| t == "bash") {
            return true;
        }
        // The screen, read-only. Old agent.md files predate the tool; an
        // agent that may look at web pages may look at the screen.
        tool == "screenshot" && self.allowlist.iter().any(|t| t == "browser")
    }
}

fn parse_bool(value: &str) -> bool {
    matches!(value, "true" | "yes" | "1")
}

pub fn validate_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 64 {
        bail!("bad agent id");
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("agent id must be [A-Za-z0-9_-]");
    }
    Ok(())
}
