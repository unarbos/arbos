//! Box configuration from the environment the relay injects (sandbox-image.md §4), and the
//! per-workspace view of it. One box per user (design D70): the box holds one workspace per
//! project under `<work_root>/<project>`, each with its own state dir, loopback ports, pi
//! session, subagents, memory DB, and history index.

use std::path::PathBuf;

use serde_json::Value;

use crate::isolation::RunAs;
use crate::settings::{DecisionConfig, DecisionProvider};

/// Decision model the group-chat responder decision uses unless the project says otherwise
/// (`ARBOS_DECISION_SCORER`); TypeSafe Jev on OpenRouter's Decisions API.
pub const DEFAULT_DECISION_SCORER: &str = "typesafe/jev-1.14";
/// Yes-probability at or above which an agent replies (`ARBOS_DECISION_THRESHOLD`).
pub const DEFAULT_DECISION_THRESHOLD: f64 = 0.5;

fn env_or(k: &str, d: &str) -> String {
    std::env::var(k)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| d.to_owned())
}

/// Box-local override of the code-mode flag: `<work_root>/.arbos/code-mode` holding `on` or `off`.
pub const CODE_MODE_FILE: &str = "code-mode";

/// `on` / `off` / `1` / `0` / `true` / `false` / `yes` / `no` → `Some(bool)`; anything else → `None`.
pub fn parse_switch(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "on" | "1" | "true" | "yes" => Some(true),
        "off" | "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

/// Box-local override of the stable-prefix flag: `<work_root>/.arbos/stable-prefix` holding `on` or `off`.
pub const STABLE_PREFIX_FILE: &str = "stable-prefix";
/// Box-local override of the send tool (duplex step 1): `<work_root>/.arbos/send-tool`, `on`, `off` or `strict`.
pub const SEND_TOOL_FILE: &str = "send-tool";
/// Box-local override of background jobs (duplex step 2): `<work_root>/.arbos/jobs`, `on` or `off`.
pub const JOBS_FILE: &str = "jobs";
/// Box-local override of act→job conversion (duplex step 4): `<work_root>/.arbos/act-to-job`, `on` or `off`.
pub const ACT_TO_JOB_FILE: &str = "act-to-job";

/// Act→job conversion (duplex step 4, Jacob 2026-10-25 16:01 UTC): an `act` turn still running after
/// `act_to_job_after()` that has used a web tool or more than `act_to_job_tool_calls()` tool calls
/// posts its ack at once and hands the rest to a job, freeing the lock. `ARBOS_ACT_TO_JOB=on|off`,
/// default **on**.
pub fn act_to_job_on(override_file: &std::path::Path) -> bool {
    switch_on("ARBOS_ACT_TO_JOB", override_file, true)
}

/// Override file for the act→job threshold: `.arbos/act-to-job-secs` (a number of seconds), in the
/// project's state dir first, then box-wide.
pub const ACT_TO_JOB_SECS_FILE: &str = "act-to-job-secs";
/// Default seconds an act turn may run before it is converted. 6 → 12 (Jacob, 2026-10-26 03:06
/// UTC, cost knob 2): a one-search ask stays inline on the talking model; Hezbola's "Search them
/// online" was converted at 6.6 s during its only tool call and paid a Sonnet worker for it.
pub const ACT_TO_JOB_SECS_DEFAULT: u64 = 12;

/// Seconds an act turn may run before it is converted: override files, then
/// `ARBOS_ACT_TO_JOB_SECS`, then `ACT_TO_JOB_SECS_DEFAULT`. Read per turn (an A/B flips it with
/// a file, no restart).
pub fn act_to_job_after_with(files: &[std::path::PathBuf]) -> std::time::Duration {
    let from_file = files.iter().find_map(|f| {
        std::fs::read_to_string(f)
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
    });
    let secs = from_file
        .or_else(|| {
            std::env::var("ARBOS_ACT_TO_JOB_SECS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
        })
        .unwrap_or(ACT_TO_JOB_SECS_DEFAULT);
    std::time::Duration::from_secs(secs)
}

/// Override file for the research-job worker model: `.arbos/research-worker-model` holding a model
/// id, or `off` to run research jobs on the subagent model like every other kind.
pub const RESEARCH_WORKER_MODEL_FILE: &str = "research-worker-model";
/// Default model for `research` jobs (Jacob, 2026-10-26 03:06 UTC, cost knob 1): the worker lane
/// was 31 Sonnet calls / $0.76 of a $5.14 run; research (web pages → a comparison) is what the
/// talking model does well. `code` and the rest keep the subagent model.
pub const RESEARCH_WORKER_MODEL_DEFAULT: &str = "anthropic/claude-haiku-5.5";

/// The model a `research` job's worker runs on: override files (project, then box), then
/// `ARBOS_RESEARCH_WORKER_MODEL`, then the default; `off` / empty → `None` (use the subagent model).
pub fn research_worker_model_with(files: &[std::path::PathBuf]) -> Option<String> {
    let raw = files
        .iter()
        .find_map(|f| {
            std::fs::read_to_string(f)
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
        })
        .or_else(|| std::env::var("ARBOS_RESEARCH_WORKER_MODEL").ok())
        .unwrap_or_else(|| RESEARCH_WORKER_MODEL_DEFAULT.to_owned());
    let v = raw.trim();
    if v.is_empty() || parse_switch(v) == Some(false) {
        None
    } else {
        Some(v.to_owned())
    }
}

/// The worker model for a job of `kind`: `research` takes the research worker model when one is
/// set, everything else the subagent model.
pub fn worker_model_for(kind: &str, research: Option<&str>, subagent: &str) -> String {
    match (kind, research) {
        ("research", Some(m)) => m.to_owned(),
        _ => subagent.to_owned(),
    }
}

/// Tool calls beyond which an act turn counts as long work (`ARBOS_ACT_TO_JOB_TOOL_CALLS`, default 3).
pub fn act_to_job_tool_calls() -> u32 {
    std::env::var("ARBOS_ACT_TO_JOB_TOOL_CALLS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3)
}

/// Background jobs as tools (duplex step 2, `docs/duplex-agent-design.md` §2.3): `start_job` /
/// `list_jobs` are offered and the router prompt carries the jobs section. `ARBOS_JOBS=on|off`,
/// default **on**. Off: the hooks refuse and the prompt is byte-identical to before.
pub fn jobs_on(override_file: &std::path::Path) -> bool {
    switch_on("ARBOS_JOBS", override_file, true)
}

/// Running or queued jobs per chat (`ARBOS_JOB_MAX_PER_CHAT`, default 3; design D4).
pub fn job_max_per_chat() -> usize {
    std::env::var("ARBOS_JOB_MAX_PER_CHAT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3)
}

/// Running or queued jobs per agent across its chats (`ARBOS_JOB_MAX_PER_AGENT`, default 6).
pub fn job_max_per_agent() -> usize {
    std::env::var("ARBOS_JOB_MAX_PER_AGENT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6)
}

/// How the main agent's words reach the chat (duplex design §2.1, `docs/duplex-agent-design.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendTool {
    /// Streamed prose is the reply, as before the duplex work.
    Off,
    /// Only `send_message` and `route.reply` post; leftover prose goes to T3. A turn that would
    /// otherwise leave the user with nothing posts its leftover prose once (counted as a fallback).
    On,
    /// As `On`, with no fallback: leftover prose is never posted.
    Strict,
}

pub fn parse_send_tool(s: &str) -> Option<SendTool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "strict" => Some(SendTool::Strict),
        other => parse_switch(other).map(|on| if on { SendTool::On } else { SendTool::Off }),
    }
}

/// `ARBOS_SEND_TOOL=on|off|strict` (default **on**); the box-local file wins when valid.
pub fn send_tool_mode(override_file: &std::path::Path) -> SendTool {
    if let Some(v) = std::fs::read_to_string(override_file)
        .ok()
        .and_then(|s| parse_send_tool(&s))
    {
        return v;
    }
    std::env::var("ARBOS_SEND_TOOL")
        .ok()
        .and_then(|s| parse_send_tool(&s))
        .unwrap_or(SendTool::On)
}
/// Box-local override of the compact memory pack: `<work_root>/.arbos/memory-pack-compact`, `on` or `off`.
pub const MEMORY_PACK_COMPACT_FILE: &str = "memory-pack-compact";
/// Box-local override of the fast memory walk: `<work_root>/.arbos/memory-walk-fast`, `on` or `off`.
pub const MEMORY_WALK_FAST_FILE: &str = "memory-walk-fast";

/// A box-wide experiment switch: the box-local `override_file` wins when it holds a valid value (so one
/// test box can run the other arm), else the environment variable `env` (the relay passes its own
/// through, so one relay setting switches the fleet), else `default`.
pub fn switch_on(env: &str, override_file: &std::path::Path, default: bool) -> bool {
    if let Some(v) = std::fs::read_to_string(override_file)
        .ok()
        .and_then(|s| parse_switch(&s))
    {
        return v;
    }
    std::env::var(env)
        .ok()
        .and_then(|s| parse_switch(&s))
        .unwrap_or(default)
}

/// Code mode (D86): the main agent is told to batch and filter tool work in `codemode` scripts, with
/// the script guardrails on. `ARBOS_CODE_MODE=on|off`. Default **off** (the eval showed no win).
pub fn code_mode_on(override_file: &std::path::Path) -> bool {
    switch_on("ARBOS_CODE_MODE", override_file, false)
}

/// Stable prefix (D87): the project-memory block of the system prompt is refreshed at most every
/// 30 min, and the working set / per-turn skills ride inside the user turn (only when they changed)
/// instead of as system-prompt sections, so the cached prefix (tools + system prompt + earlier
/// turns) is reused instead of rewritten. `ARBOS_STABLE_PREFIX=on|off`. Default **on** since the
/// eval of 2026-10-20 (multi-turn chats: −23 % cost, −25 % wall time, 0 prefix rewrites; single-turn
/// chats: tie; quality held).
pub fn stable_prefix_on(override_file: &std::path::Path) -> bool {
    switch_on("ARBOS_STABLE_PREFIX", override_file, true)
}

/// Compact memory pack (D77 follow-up to D87): the per-turn `[Memory retrieved …]` block gets
/// per-kind budgets and a cap, page-carded tool outputs are packed as card + facts, repeats and
/// leaves already in the prompt's project-memory block are dropped. `ARBOS_MEMORY_PACK_COMPACT=on|off`.
/// Default **on** since the eval of 2026-10-20 (code-mode eval set, 60 single-turn chats: first-call
/// cache writes −39 %, main-agent cost per turn −16 %; 40 multi-turn turns: −54 % / −26 %; quality
/// held, block p90 4.7k → 2.0k tokens).
pub fn memory_pack_compact_on(override_file: &std::path::Path) -> bool {
    switch_on("ARBOS_MEMORY_PACK_COMPACT", override_file, true)
}

/// Fast memory walk (D77 follow-up, 2026-10-21): threads with no stemmed content word in common
/// with the question (in a leaf or an index card) are not sent to Jev; every Jev score is cached
/// per (card, question words, day); a small candidate set is scored in one round instead of
/// three. Keep rules and thresholds are unchanged. `ARBOS_MEMORY_WALK_FAST=on|off`, read at every
/// walk. Default **on**; `off` restores the D77 walk exactly.
pub fn memory_walk_fast_on(override_file: &std::path::Path) -> bool {
    switch_on("ARBOS_MEMORY_WALK_FAST", override_file, true)
}

/// Box-level settings: what every workspace shares.
#[derive(Debug, Clone)]
pub struct BoxConfig {
    /// Owner of the box; `None` until `/box/hello` answers (legacy env without it).
    pub user_id: Option<String>,
    pub relay_url: String,
    pub box_token: String,
    pub mcp_token: String,
    /// `/work`: workspaces live in `<work_root>/<project>`; box-level state in `<work_root>/.arbos`.
    pub work_root: PathBuf,
    pub bind: String,
    /// The box HTTP port (health, history, browser stream); the relay's preview URL points here.
    pub port: u16,
    pub local_box: bool,
    pub pi_bin: String,
    pub prime_bin: String,
    pub claude_bin: String,
    pub pi_ext_override: Option<PathBuf>,
    pub device_id: String,
    pub default_talking_model: String,
    pub default_subagent_model: String,
    /// Pre-D70 boxes were created with `ARBOS_PROJECT_ID`; the supervisor treats it as the first
    /// workspace when the relay lists it.
    pub legacy_project_id: Option<String>,
}

impl BoxConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let work_root = PathBuf::from(env_or("ARBOS_WORK_DIR", "/work"));
        let port: u16 = env_or("ARBOS_PORT", "7788").parse().unwrap_or(7788);
        let sandbox_id = std::env::var("DAYTONA_SANDBOX_ID").ok();
        let local_box = env_or("ARBOS_LOCAL_BOX", "0") == "1";
        let user_id = std::env::var("ARBOS_USER_ID")
            .ok()
            .filter(|s| !s.is_empty());
        let device_id = match (&sandbox_id, &user_id) {
            (Some(id), _) => format!("box:{id}"),
            (None, Some(u)) => format!("box:local-{u}"),
            (None, None) => format!("box:local-{}", env_or("ARBOS_PROJECT_ID", "box")),
        };
        Ok(Self {
            user_id,
            relay_url: env_or("ARBOS_RELAY_URL", "http://127.0.0.1:7700")
                .trim_end_matches('/')
                .to_owned(),
            box_token: env_or("ARBOS_BOX_TOKEN", ""),
            mcp_token: env_or("ARBOS_MCP_TOKEN", ""),
            work_root,
            bind: env_or("ARBOS_BIND", "0.0.0.0"),
            port,
            local_box,
            pi_bin: env_or("ARBOS_PI_BIN", "pi"),
            prime_bin: env_or("ARBOS_PRIME_BIN", "prime-agent"),
            claude_bin: env_or("ARBOS_CLAUDE_BIN", "claude"),
            pi_ext_override: std::env::var("ARBOS_PI_EXT").ok().map(PathBuf::from),
            device_id,
            default_talking_model: env_or("ARBOS_TALKING_MODEL", "anthropic/claude-haiku-5.5"),
            default_subagent_model: env_or("ARBOS_SUBAGENT_MODEL", "anthropic/claude-sonnet-5.5"),
            legacy_project_id: std::env::var("ARBOS_PROJECT_ID")
                .ok()
                .filter(|s| !s.is_empty()),
        })
    }

    /// Box-level state (relay-url file, box logs).
    pub fn box_state_dir(&self) -> PathBuf {
        self.work_root.join(".arbos")
    }

    /// Loopback ports for workspace number `index` (MCP, hook API, model stub). A Daytona box
    /// uses 7800 + 10·index; a local box hangs them off its main port so several boxes share a host.
    fn loopback_base(&self, index: u16) -> u16 {
        if self.local_box {
            self.port + 10 + 10 * index
        } else {
            7800 + 10 * index
        }
    }

    /// The per-workspace config for one project.
    pub fn workspace(&self, project: &Value, index: u16) -> Config {
        let project_id = project
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("project")
            .to_owned();
        let models = project.get("models").cloned().unwrap_or(Value::Null);
        let slot = |k: &str, d: &str| -> String {
            models
                .pointer(&format!("/{k}/id"))
                .and_then(Value::as_str)
                .unwrap_or(d)
                .to_owned()
        };
        let work_dir = self.work_root.join(&project_id);
        let state_dir = work_dir.join(".arbos");
        let base = self.loopback_base(index);
        let talking = slot("talking_model", &self.default_talking_model);
        Config {
            project_id: project_id.clone(),
            project_name: project
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(&project_id)
                .to_owned(),
            relay_url: self.relay_url.clone(),
            box_token: self.box_token.clone(),
            mcp_token: self.mcp_token.clone(),
            mcp_port: base + 1,
            hook_port: base + 2,
            proxy_port: base + 3,
            sites_port: base + 4,
            browser_stream_port: 9223 + index,
            decision_model: slot("decision_model", &talking),
            decision_cfg: decision_cfg_of(&models),
            talking_model: talking,
            subagent_model: slot("subagent_model", &self.default_subagent_model),
            instructions: project
                .get("instructions")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            pi_bin: self.pi_bin.clone(),
            prime_bin: self.prime_bin.clone(),
            claude_bin: self.claude_bin.clone(),
            pi_ext_path: self.pi_ext_override.clone().unwrap_or_else(|| {
                state_dir
                    .join("pi")
                    .join("extensions")
                    .join("arbos-pi-ext")
                    .join("index.ts")
            }),
            device_id: self.device_id.clone(),
            state_dir,
            work_dir,
            run_as: None,
        }
    }
}

/// The responder-decision settings of a project's `models.decision_model` slot, with the box
/// environment (`ARBOS_DECISION_PROVIDER` = `jev` | `llm`, `ARBOS_DECISION_SCORER`,
/// `ARBOS_DECISION_THRESHOLD`) as the defaults.
fn decision_cfg_of(models: &Value) -> DecisionConfig {
    let env_provider = DecisionProvider::parse(&env_or("ARBOS_DECISION_PROVIDER", "jev"))
        .unwrap_or(DecisionProvider::Jev);
    let env_threshold = env_or("ARBOS_DECISION_THRESHOLD", "")
        .parse::<f64>()
        .ok()
        .filter(|t| (0.0..=1.0).contains(t))
        .unwrap_or(DEFAULT_DECISION_THRESHOLD);
    DecisionConfig {
        provider: models
            .pointer("/decision_model/provider")
            .and_then(Value::as_str)
            .and_then(DecisionProvider::parse)
            .unwrap_or(env_provider),
        scorer: models
            .pointer("/decision_model/scorer")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| env_or("ARBOS_DECISION_SCORER", DEFAULT_DECISION_SCORER)),
        threshold: models
            .pointer("/decision_model/threshold")
            .and_then(Value::as_f64)
            .filter(|t| (0.0..=1.0).contains(t))
            .unwrap_or(env_threshold),
        memory_tree: models
            .pointer("/decision_model/memory_tree")
            .and_then(Value::as_str)
            .and_then(crate::tree::TreeMode::parse)
            .unwrap_or_else(|| {
                crate::tree::TreeMode::parse(&env_or("ARBOS_MEMORY_TREE", "on"))
                    .unwrap_or(crate::tree::TreeMode::On)
            }),
    }
}

/// One workspace (= one project) as the agent code sees it.
#[derive(Debug, Clone)]
pub struct Config {
    pub project_id: String,
    pub project_name: String,
    pub relay_url: String,
    pub box_token: String,
    pub mcp_token: String,
    pub state_dir: PathBuf,
    pub work_dir: PathBuf,
    pub mcp_port: u16,
    pub hook_port: u16,
    pub proxy_port: u16,
    /// Loopback preview of `sites/<app>/` with the edge's rules (#2060).
    pub sites_port: u16,
    /// agent-browser's JPEG stream port for this workspace (9223 + index); one browser per workspace.
    pub browser_stream_port: u16,
    pub talking_model: String,
    pub subagent_model: String,
    pub decision_model: String,
    /// Responder decision: provider switch, decision-model id, threshold (`decision_model` slot).
    pub decision_cfg: DecisionConfig,
    pub instructions: String,
    pub pi_bin: String,
    pub prime_bin: String,
    pub claude_bin: String,
    pub pi_ext_path: PathBuf,
    pub device_id: String,
    /// The Unix identity this workspace's children run as (`None` = agentd's own, local boxes).
    pub run_as: Option<RunAs>,
}

impl Config {
    /// pi provider name: `openrouter` (built-in behaviours over our proxy) unless overridden.
    pub fn pi_provider(&self) -> String {
        env_or("ARBOS_PI_PROVIDER", "openrouter")
    }

    pub fn proxy_base(&self) -> String {
        format!("http://127.0.0.1:{}", self.proxy_port)
    }

    pub fn hook_base(&self) -> String {
        format!("http://127.0.0.1:{}", self.hook_port)
    }

    pub fn mcp_url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.mcp_port)
    }

    /// The bearer token subagent sessions present to the MCP server and hooks. Derived from
    /// the main token so the main session's token never reaches a subagent process; the MCP
    /// server tells the two apart to refuse main-only tools (D69: subagents never talk
    /// project-to-project).
    pub fn subagent_mcp_token(&self) -> String {
        use sha2::Digest;
        let digest = sha2::Sha256::digest(format!("subagent:{}", self.mcp_token).as_bytes());
        format!("sub_{}", hex::encode(&digest[..16]))
    }

    /// The MCP token a session of `role` (`main` or `subagent`) is given.
    pub fn mcp_token_for_role(&self, role: &str) -> String {
        if role == "main" {
            self.mcp_token.clone()
        } else {
            self.subagent_mcp_token()
        }
    }

    pub fn pi_dir(&self) -> PathBuf {
        self.state_dir.join("pi")
    }

    pub fn pi_sessions_dir(&self) -> PathBuf {
        self.pi_dir().join("sessions")
    }

    pub fn prime_dir(&self) -> PathBuf {
        self.state_dir.join("prime")
    }

    pub fn blobs_dir(&self) -> PathBuf {
        self.state_dir.join("blobs")
    }

    pub fn audit_path(&self) -> PathBuf {
        self.state_dir.join("audit.jsonl")
    }

    /// Code mode for this workspace's main agent (D86). Read at every pi (re)spawn, so a toggle
    /// needs a pi respawn, not a box restart.
    pub fn code_mode(&self) -> bool {
        code_mode_on(&self.box_flag_file(CODE_MODE_FILE))
    }

    /// Background jobs (duplex step 2); read at every pi (re)spawn like code mode (the prompt
    /// section rides in the cached prefix) and at every `start_job` call.
    pub fn jobs(&self) -> bool {
        jobs_on(&self.box_flag_file(JOBS_FILE))
    }

    /// Act→job conversion (duplex step 4); read per turn.
    pub fn act_to_job(&self) -> bool {
        act_to_job_on(&self.box_flag_file(ACT_TO_JOB_FILE))
    }

    /// Seconds before an act turn becomes a job (cost knob 2); project file, box file, env, default.
    pub fn act_to_job_after(&self) -> std::time::Duration {
        act_to_job_after_with(&self.knob_files(ACT_TO_JOB_SECS_FILE))
    }

    /// The model `research` jobs run on (cost knob 1); `None` = the subagent model.
    pub fn research_worker_model(&self) -> Option<String> {
        research_worker_model_with(&self.knob_files(RESEARCH_WORKER_MODEL_FILE))
    }

    /// A knob's override files in precedence order: this project's `.arbos/<name>` (so an A/B can
    /// run two projects on one box with different settings), then the box-wide file.
    fn knob_files(&self, name: &str) -> Vec<PathBuf> {
        vec![self.state_dir.join(name), self.box_flag_file(name)]
    }

    /// Send tool (duplex step 1): read per turn, so a toggle needs no respawn.
    pub fn send_tool(&self) -> SendTool {
        send_tool_mode(&self.box_flag_file(SEND_TOOL_FILE))
    }

    /// Stable prefix for this workspace's main agent (D87); read at every pi (re)spawn.
    pub fn stable_prefix(&self) -> bool {
        stable_prefix_on(&self.box_flag_file(STABLE_PREFIX_FILE))
    }

    /// Compact per-turn memory pack; read at every walk, so a toggle needs no restart.
    pub fn memory_pack_compact(&self) -> bool {
        memory_pack_compact_on(&self.box_flag_file(MEMORY_PACK_COMPACT_FILE))
    }

    /// Fast memory walk (prefilter, score cache, one-round scoring); read at every walk.
    pub fn memory_walk_fast(&self) -> bool {
        memory_walk_fast_on(&self.box_flag_file(MEMORY_WALK_FAST_FILE))
    }

    /// Both per-walk switches at once.
    pub fn walk_options(&self) -> crate::tree::WalkOptions {
        crate::tree::WalkOptions {
            compact: self.memory_pack_compact(),
            fast: self.memory_walk_fast(),
        }
    }

    /// `<work_root>/.arbos/<name>`: a box-wide flag file an operator can set for one box.
    fn box_flag_file(&self, name: &str) -> PathBuf {
        let root = self.work_dir.parent().unwrap_or(&self.work_dir);
        root.join(".arbos").join(name)
    }

    /// Browser settings per workspace: own profile, screenshots, stream port (the image's
    /// box-wide defaults would collide between workspaces and are unwritable to their users).
    pub fn browser_env(&self) -> Vec<(String, String)> {
        vec![
            (
                "AGENT_BROWSER_PROFILE".into(),
                self.state_dir.join("browser").display().to_string(),
            ),
            (
                "AGENT_BROWSER_SCREENSHOT_DIR".into(),
                self.state_dir.join("shots").display().to_string(),
            ),
            (
                "AGENT_BROWSER_STREAM_PORT".into(),
                self.browser_stream_port.to_string(),
            ),
            // Loopback preview of `sites/<app>/` with the edge's serving rules (#2060).
            (
                "ARBOS_SITES_PREVIEW_URL".into(),
                format!("http://127.0.0.1:{}/sites/", self.sites_port),
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cost knobs (Jacob 2026-10-26): per-project file > box file > env > default; `off` turns
    /// the research model off; the threshold default is 12 s.
    #[test]
    fn cost_knob_precedence() {
        let dir = std::env::temp_dir().join(format!("arbos-knobs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let proj = dir.join("p").join(RESEARCH_WORKER_MODEL_FILE);
        let bx = dir.join("b").join(RESEARCH_WORKER_MODEL_FILE);
        std::fs::create_dir_all(proj.parent().unwrap()).unwrap();
        std::fs::create_dir_all(bx.parent().unwrap()).unwrap();
        let files = vec![proj.clone(), bx.clone()];
        if std::env::var("ARBOS_RESEARCH_WORKER_MODEL").is_err() {
            assert_eq!(
                research_worker_model_with(&files).as_deref(),
                Some(RESEARCH_WORKER_MODEL_DEFAULT)
            );
        }
        std::fs::write(&bx, "off\n").unwrap();
        assert_eq!(research_worker_model_with(&files), None);
        std::fs::write(&proj, "anthropic/claude-sonnet-5.5\n").unwrap();
        assert_eq!(
            research_worker_model_with(&files).as_deref(),
            Some("anthropic/claude-sonnet-5.5"),
            "the project file wins over the box file"
        );
        std::fs::write(&proj, "  \n").unwrap();
        assert_eq!(
            research_worker_model_with(&files),
            None,
            "blank project file falls through to the box file (off)"
        );
        assert_eq!(
            worker_model_for(
                "research",
                Some("anthropic/claude-haiku-5.5"),
                "anthropic/claude-sonnet-5.5"
            ),
            "anthropic/claude-haiku-5.5"
        );
        assert_eq!(
            worker_model_for(
                "code",
                Some("anthropic/claude-haiku-5.5"),
                "anthropic/claude-sonnet-5.5"
            ),
            "anthropic/claude-sonnet-5.5"
        );
        assert_eq!(
            worker_model_for("research", None, "anthropic/claude-sonnet-5.5"),
            "anthropic/claude-sonnet-5.5"
        );
        let sp = dir.join("p").join(ACT_TO_JOB_SECS_FILE);
        let sb = dir.join("b").join(ACT_TO_JOB_SECS_FILE);
        let sfiles = vec![sp.clone(), sb.clone()];
        if std::env::var("ARBOS_ACT_TO_JOB_SECS").is_err() {
            assert_eq!(
                act_to_job_after_with(&sfiles).as_secs(),
                ACT_TO_JOB_SECS_DEFAULT
            );
            assert_eq!(ACT_TO_JOB_SECS_DEFAULT, 12);
        }
        std::fs::write(&sb, "6\n").unwrap();
        assert_eq!(act_to_job_after_with(&sfiles).as_secs(), 6);
        std::fs::write(&sp, "20").unwrap();
        assert_eq!(act_to_job_after_with(&sfiles).as_secs(), 20);
        std::fs::write(&sp, "soon").unwrap();
        assert_eq!(
            act_to_job_after_with(&sfiles).as_secs(),
            6,
            "a garbage project file falls through"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn switch_words() {
        assert_eq!(parse_switch(" ON\n"), Some(true));
        assert_eq!(parse_switch("off"), Some(false));
        assert_eq!(parse_switch("1"), Some(true));
        assert_eq!(parse_switch("maybe"), None);
        assert_eq!(parse_switch(""), None);
    }

    /// The box-local file wins over the environment; a garbage file falls back to the env; the
    /// default is off.
    #[test]
    fn code_mode_file_overrides_env() {
        let dir = std::env::temp_dir().join(format!("arbos-cm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join(CODE_MODE_FILE);
        let _ = std::fs::remove_file(&f);
        // No env in the test process unless the runner set one: the default is off.
        if std::env::var("ARBOS_CODE_MODE").is_err() {
            assert!(!code_mode_on(&f));
        }
        std::fs::write(&f, "on\n").unwrap();
        assert!(code_mode_on(&f));
        std::fs::write(&f, "off").unwrap();
        assert!(!code_mode_on(&f));
        std::fs::write(&f, "banana").unwrap();
        if std::env::var("ARBOS_CODE_MODE").is_err() {
            assert!(!code_mode_on(&f));
        }
        let sp = dir.join(STABLE_PREFIX_FILE);
        if std::env::var("ARBOS_STABLE_PREFIX").is_err() {
            assert!(stable_prefix_on(&sp), "stable prefix is on by default");
        }
        std::fs::write(&sp, "off").unwrap();
        assert!(!stable_prefix_on(&sp), "the box file turns it off");
        std::fs::write(&sp, "on").unwrap();
        assert!(stable_prefix_on(&sp));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
