//! A tool is a value with two phases.
//!
//! `plan` is cheap and pure: it looks at the arguments and says what the call
//! will touch ([`Access`]) and whether a human must be involved. The
//! scheduler uses it to decide what may run together and what may start early.
//!
//! `run` is the effect. It gets a [`RunCx`] with the cwd, a cancellation
//! token, and the shared services (jobs, grep, kernel hooks).
//!
//! The [`Registry`] owns the tools. A [`View`] is the registry seen through
//! one agent's allowlist: only those schemas go to the model, only those
//! names resolve.

use anyhow::{Result, anyhow};
use arbos_core::{Agent, Place};
use serde_json::{Value, json};
use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

use crate::access::Access;
use crate::tools::{Grep, Hooks};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What `plan` decides.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub access: Access,
    /// Needs a human before or during the run (approval, a question).
    /// Scheduled alone and in order so prompts never interleave.
    pub interactive: bool,
}

impl Plan {
    pub fn access(access: Access) -> Self {
        Self {
            access,
            interactive: false,
        }
    }

    pub fn interactive(mut self) -> Self {
        self.interactive = true;
        self
    }
}

pub struct PlanCx<'a> {
    /// The place. Every file tool is confined to it (see `fs::confine`).
    pub root: &'a Path,
    pub cwd: &'a Path,
    pub agent: &'a Agent,
}

impl PlanCx<'_> {
    /// A path argument as the tool will use it, or an error when it would
    /// leave the place. Planning refuses so the call never runs.
    pub fn resolve(&self, path: &str) -> Result<PathBuf> {
        crate::tools::fs::confine(self.root, self.cwd, path)
    }

    /// `resolve` for a tool that will write there. `notes.md`,
    /// `docs/project-context.md`, and `archived.md` belong to the main
    /// chat: a child's write is refused here, before it runs. A
    /// coordinator writes the project store and nothing else — code is a
    /// worker's job.
    pub fn resolve_write(&self, path: &str) -> Result<PathBuf> {
        let resolved = self.resolve(path)?;
        // The store's rules are the place's, whichever root confines this
        // agent: a worktree child reaches `.arbos/` too (qa-035), and
        // the page stays root's.
        let store = crate::tools::fs::store_dir(self.root);
        let place_root = store.parent().unwrap_or(self.root);
        if arbos_core::store::is_root_owned(place_root, &resolved)
            && !arbos_core::store::may_write(self.agent)
        {
            anyhow::bail!("{}", arbos_core::store::REFUSAL);
        }
        if self.agent.role.as_deref() == Some(arbos_core::project::COORDINATOR)
            && !arbos_core::store::is_store_path(place_root, &resolved)
        {
            anyhow::bail!("{}", arbos_core::store::COORDINATOR_REFUSAL);
        }
        // The project page (.arbos/notes.md) guard lives in the store
        // module's root-owned check (#103); nothing more here.
        Ok(resolved)
    }

    /// Lexical resolution with no confinement: for `bash`, whose command can
    /// `cd` anywhere regardless, so gating its `cwd` argument would only
    /// mislead.
    pub fn resolve_unconfined(&self, path: &str) -> PathBuf {
        crate::tools::fs::resolve(self.cwd, path)
    }
}

/// Everything a running tool may need. Cloned per call.
#[derive(Clone)]
pub struct RunCx {
    pub place: Place,
    pub agent: Agent,
    pub cwd: PathBuf,
    pub call_id: String,
    /// The model step within the turn this call belongs to (1-based);
    /// the turn sets it before each model call. On every event the step
    /// writes.
    pub step: u64,
    pub cancel: CancellationToken,
    pub grep: Arc<dyn Grep>,
    pub hooks: Arc<dyn Hooks>,
    /// How long `bash` stays attached before handing off to a job.
    pub bash_wait_ms: u64,
    /// Reply budget this turn was started with. `say` spends it.
    pub hops: u8,
    /// What `search` and `fetch` may use: the custom endpoint from
    /// config, and the model provider (OpenRouter's web plugin).
    pub web: Arc<WebCfg>,
    /// `true` once this turn's checkpoint has its working tree saved (or
    /// gave up). A tool that writes waits on it first: the tree is taken
    /// beside the turn, and a first tool call that landed before `add -A`
    /// finished was in the "before" tree — a rewind then restored the
    /// turn's own file and said restored (qal-j17). `None`: nothing to
    /// wait for.
    pub tree_ready: Option<tokio::sync::watch::Receiver<bool>>,
}

impl RunCx {
    /// Where the file tools may reach. The place, or — for a child that
    /// works in its own worktree under `.arbos/worktrees/` — that worktree,
    /// so an isolated child cannot edit its parent's checkout by path.
    pub fn root(&self) -> &Path {
        confinement_root(&self.place, &self.cwd)
    }
}

/// See [`RunCx::root`].
pub fn confinement_root<'a>(place: &'a Place, cwd: &'a Path) -> &'a Path {
    if cwd.starts_with(place.worktrees_dir()) {
        cwd
    } else {
        place.path()
    }
}

/// Web access settings, from `config.toml` and the environment.
#[derive(Debug, Default, Clone)]
pub struct WebCfg {
    pub search_url: Option<String>,
    pub search_key: Option<String>,
    /// The model provider's base URL and key; OpenRouter's web plugin
    /// answers searches when no search backend is configured.
    pub api_base: String,
    pub api_key: Option<String>,
    /// `search_model` from config; empty = the tool's default.
    pub model: String,
}

#[derive(Debug)]
pub struct ToolOut {
    pub body: String,
    /// Paths the call touched, for the UI and the transcript.
    pub paths: Vec<String>,
    /// Child agent id, when the call spawned one.
    pub child: Option<String>,
    /// Image files for the model to look at. Projected as image parts.
    pub images: Vec<String>,
    /// Display diff for the transcript card. The model never sees this.
    pub diff: Option<String>,
    /// The turn ends after this call, with this notice: the tool parked
    /// the agent (an `ask` waits for the user as a file; the answer starts
    /// the next turn).
    pub park: Option<String>,
}

impl ToolOut {
    /// End the turn after this call. `body` is what the model would see if
    /// it ran on — it does not; `why` is the notice on the transcript.
    pub fn parked(body: impl Into<String>, why: impl Into<String>) -> Self {
        Self {
            park: Some(why.into()),
            ..Self::text(body)
        }
    }

    pub fn text(body: impl Into<String>) -> Self {
        Self {
            body: body.into(),
            paths: Vec::new(),
            child: None,
            images: Vec::new(),
            diff: None,
            park: None,
        }
    }

    pub fn with_paths(body: impl Into<String>, paths: Vec<String>) -> Self {
        Self {
            body: body.into(),
            paths,
            child: None,
            images: Vec::new(),
            diff: None,
            park: None,
        }
    }

    pub fn with_diff(mut self, diff: String) -> Self {
        if !diff.is_empty() {
            self.diff = Some(diff);
        }
        self
    }
}

pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    /// OpenAI function schema.
    fn schema(&self) -> Value;
    /// Pure. No I/O. Reads the arguments only.
    fn plan(&self, cx: &PlanCx, args: &Value) -> Result<Plan>;
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>>;
}

/// Run a synchronous body on the blocking pool.
pub fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T> + Send + 'static,
) -> BoxFuture<'static, Result<T>> {
    Box::pin(async move {
        tokio::task::spawn_blocking(f)
            .await
            .map_err(|e| anyhow!("tool task: {e}"))?
    })
}

/// One parameter of a [`typed_schema`]: key, description, required, JSON
/// type (`"string"`, `"integer"`, `"boolean"`, `"number"`, or `"array"` of
/// strings).
pub type Param<'a> = (&'a str, &'a str, bool, &'a str);

/// Build a flat schema with a JSON type per parameter, so the model sends
/// `true` and `["a","b"]` rather than the strings a string-only schema
/// forces on it (which the tools then could not read).
pub fn typed_schema(name: &str, desc: &str, params: &[Param]) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for (key, d, req, kind) in params {
        let mut prop = if *kind == "array" {
            json!({"type": "array", "items": {"type": "string"}})
        } else {
            json!({"type": kind})
        };
        if !d.is_empty() {
            prop["description"] = json!(d);
        }
        properties.insert((*key).into(), prop);
        if *req {
            required.push(*key);
        }
    }
    function_schema(name, desc, properties, required)
}

/// A boolean argument, accepting the strings a lax model may still send.
pub fn opt_bool(args: &Value, key: &str) -> Option<bool> {
    match args.get(key)? {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "1" => Some(true),
            "false" | "no" | "0" | "" => Some(false),
            _ => None,
        },
        Value::Number(n) => n.as_i64().map(|i| i != 0),
        _ => None,
    }
}

/// A list-of-strings argument: a JSON array, or one string split on commas
/// or newlines when the model sent it flat.
pub fn opt_strings(args: &Value, key: &str) -> Vec<String> {
    match args.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|x| match x {
                Value::String(s) => Some(s.clone()),
                other if !other.is_null() => Some(other.to_string()),
                _ => None,
            })
            .collect(),
        Some(Value::String(s)) => s
            .split(|c| c == ',' || c == '\n')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// Build a flat schema: every parameter is a string.
pub fn simple_schema(name: &str, desc: &str, params: &[(&str, &str, bool)]) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for (key, d, req) in params {
        let mut prop = json!({"type": "string"});
        if !d.is_empty() {
            prop["description"] = json!(d);
        }
        properties.insert((*key).into(), prop);
        if *req {
            required.push(*key);
        }
    }
    function_schema(name, desc, properties, required)
}

/// The provider's function shape. An empty `required` is left out: every
/// key of every schema rides in every model call, so nothing empty rides.
fn function_schema(
    name: &str,
    desc: &str,
    properties: serde_json::Map<String, Value>,
    required: Vec<&str>,
) -> Value {
    let mut parameters = json!({"type": "object", "properties": properties});
    if !required.is_empty() {
        parameters["required"] = json!(required);
    }
    json!({
        "type": "function",
        "function": {"name": name, "description": desc, "parameters": parameters}
    })
}

/// Argument helpers shared by tool impls.
pub fn req<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    match args.get(key) {
        Some(Value::String(s)) => Ok(s),
        // A number or bool where a string belongs is still an answer.
        Some(v) if !v.is_null() && !v.is_object() && !v.is_array() => {
            Err(anyhow!("{key} must be a string, got {v}"))
        }
        _ => {
            let sent: Vec<&str> = args
                .as_object()
                .map(|o| o.keys().map(String::as_str).collect())
                .unwrap_or_default();
            if sent.is_empty() {
                Err(anyhow!("missing {key}: the call had no arguments"))
            } else {
                Err(anyhow!("missing {key}; the call had: {}", sent.join(", ")))
            }
        }
    }
}

pub fn opt_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

pub fn opt_u64(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_i64().map(|i| i as u64))
            .or_else(|| v.as_str()?.parse().ok())
    })
}

/// All tools the kernel ships plus any the host registers.
#[derive(Clone, Default)]
pub struct Registry {
    tools: Vec<Arc<dyn Tool>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// The engine's own tools.
    pub fn builtin() -> Self {
        crate::tools::builtin()
    }

    /// Add or replace a tool by name.
    pub fn with(mut self, tool: impl Tool + 'static) -> Self {
        self.tools.retain(|t| t.name() != tool.name());
        self.tools.push(Arc::new(tool));
        self
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.tools.iter().map(|t| t.name()).collect()
    }

    /// The registry as one agent sees it.
    pub fn view(&self, agent: &Agent) -> View {
        let tools: Vec<Arc<dyn Tool>> = self
            .tools
            .iter()
            .filter(|t| agent.may(t.name()))
            .cloned()
            .collect();
        let coordinator = agent.role.as_deref() == Some(arbos_core::project::COORDINATOR);
        let schemas = tools
            .iter()
            .map(|t| {
                let schema = t.schema();
                if coordinator {
                    coordinator_schema(t.name(), schema)
                } else {
                    schema
                }
            })
            .collect();
        View {
            tools: Arc::new(tools),
            schemas: Arc::new(schemas),
        }
    }
}

/// What a coordinator reads on the editing tools and on `spawn`, ahead
/// of the tool's own description. The contract says a code change is a
/// spawn; the model still reached for `edit` on `main.py` first and met
/// the refusal (F-46). The line at the point of choice — the tool list —
/// is the one it reads when choosing.
pub const COORDINATOR_EDIT_NOTE: &str = "COORDINATOR: project store only (.arbos/notes.md, docs/, internal/, media/). Code and every other file are a worker's — on a request that changes code, call spawn first, never this; the kernel refuses it here.";
pub const COORDINATOR_SPAWN_NOTE: &str = "COORDINATOR: your first call on any request that changes code, fixes a bug, adds a feature, or runs a build or tests — before any read, grep, or edit. wait=true for a one-off, then relay its result.";

fn coordinator_schema(name: &str, mut schema: Value) -> Value {
    let note = match name {
        "write" | "edit" | "apply_patch" | "delete" => COORDINATOR_EDIT_NOTE,
        "spawn" => COORDINATOR_SPAWN_NOTE,
        _ => return schema,
    };
    if let Some(desc) = schema
        .get_mut("function")
        .and_then(|f| f.get_mut("description"))
    {
        let own = desc.as_str().unwrap_or("").to_string();
        *desc = Value::String(if own.is_empty() {
            note.to_string()
        } else {
            format!("{note} {own}")
        });
    }
    schema
}

#[cfg(test)]
mod coordinator_view_tests {
    use super::*;

    /// F-46: the coordinator edited `main.py` itself and was refused
    /// instead of spawning. The tool list it chooses from now says so on
    /// the editing tools and on spawn; a worker's list is unchanged.
    #[test]
    fn a_coordinator_reads_the_role_note_on_editing_tools_and_spawn() {
        let reg = Registry::builtin();
        let mut coord = Agent::root("root");
        coord.role = Some(arbos_core::project::COORDINATOR.to_string());
        let worker = Agent::root("w");
        let desc = |view: &View, name: &str| -> String {
            view.schemas()
                .iter()
                .find(|s| s["function"]["name"] == name)
                .map(|s| {
                    s["function"]["description"]
                        .as_str()
                        .unwrap_or("")
                        .to_string()
                })
                .unwrap_or_default()
        };
        let cv = reg.view(&coord);
        let wv = reg.view(&worker);
        for name in ["write", "edit", "apply_patch"] {
            assert!(
                desc(&cv, name).starts_with(COORDINATOR_EDIT_NOTE),
                "{name}: {}",
                desc(&cv, name)
            );
            assert!(!desc(&wv, name).contains("COORDINATOR"), "{name}");
        }
        // `spawn` is a kernel tool (see coordinator_spawn_first_e2e); the
        // note is applied by name here.
        let spawn = coordinator_schema(
            "spawn",
            serde_json::json!({"function": {"name": "spawn", "description": "Start a worker."}}),
        );
        assert_eq!(
            spawn["function"]["description"],
            format!("{COORDINATOR_SPAWN_NOTE} Start a worker.")
        );
        // Other tools keep their own words.
        assert_eq!(desc(&cv, "read"), desc(&wv, "read"));
    }
}

#[derive(Clone)]
pub struct View {
    tools: Arc<Vec<Arc<dyn Tool>>>,
    schemas: Arc<Vec<Value>>,
}

impl View {
    pub fn schemas(&self) -> &[Value] {
        &self.schemas
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.name() == name)
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

#[cfg(test)]
mod store_guard_tests {
    use super::*;

    #[test]
    fn a_child_may_not_write_the_status_page_but_root_may() {
        let dir = std::env::temp_dir().join(format!("arbos-store-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".arbos")).unwrap();
        arbos_core::store::ensure(&arbos_core::Place::new(dir.clone())).unwrap();
        let root = arbos_core::Agent::root("root");
        let mut child = arbos_core::Agent::root("kid");
        child.parent = Some(arbos_core::AgentId::new("root"));
        let for_root = PlanCx {
            root: &dir,
            cwd: &dir,
            agent: &root,
        };
        assert!(for_root.resolve_write(".arbos/notes.md").is_ok());
        assert!(for_root.resolve_write(".arbos/GOALS.md").is_ok());
        assert!(for_root.resolve_write("main.py").is_ok());
        let for_child = PlanCx {
            root: &dir,
            cwd: &dir,
            agent: &child,
        };
        for owned in [
            ".arbos/notes.md",
            ".arbos/GOALS.md",
            ".arbos/docs/project-context.md",
            ".arbos/archived.md",
        ] {
            let err = for_child.resolve_write(owned).unwrap_err();
            assert!(
                err.to_string().contains("owned by the main chat"),
                "{owned}: {err}"
            );
        }
        assert!(for_child.resolve_write(".arbos/docs/design.md").is_ok());
        assert!(for_child.resolve_write("main.py").is_ok());
    }

    #[test]
    fn a_coordinator_writes_only_the_store() {
        let dir = std::env::temp_dir().join(format!("arbos-coord-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".arbos")).unwrap();
        arbos_core::store::ensure(&arbos_core::Place::new(dir.clone())).unwrap();
        let mut root = arbos_core::Agent::root("root");
        root.role = Some(arbos_core::project::COORDINATOR.into());
        let cx = PlanCx {
            root: &dir,
            cwd: &dir,
            agent: &root,
        };
        for ok in [
            ".arbos/notes.md",
            ".arbos/docs/project-context.md",
            ".arbos/docs/design.md",
            ".arbos/internal/qa/2026-09-13.md",
            ".arbos/media/layout/a.png",
            ".arbos/archived.md",
        ] {
            assert!(cx.resolve_write(ok).is_ok(), "{ok}");
        }
        for no in [
            "main.py",
            ".arbos/agents/root/plan.md",
            ".arbos/project.toml",
        ] {
            let err = cx.resolve_write(no).unwrap_err();
            assert!(err.to_string().contains("as coordinator"), "{no}: {err}");
        }
    }
}
