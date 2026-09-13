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

    /// `resolve` for a tool that will write there. GOALS.md belongs to the
    /// main chat: a child's write is refused here, before it runs.
    pub fn resolve_write(&self, path: &str) -> Result<PathBuf> {
        let resolved = self.resolve(path)?;
        if arbos_core::goals::is_goals_path(self.root, &resolved)
            && !arbos_core::goals::may_write(self.agent)
        {
            anyhow::bail!("{}", arbos_core::goals::REFUSAL);
        }
        if arbos_core::notes::is_project_page(self.root, &resolved)
            && self.agent.id.as_str() != arbos_core::ROOT_ID
        {
            anyhow::bail!("{}", arbos_core::notes::PAGE_REFUSAL);
        }
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
        let prop = if *kind == "array" {
            json!({"type": "array", "items": {"type": "string"}, "description": d})
        } else {
            json!({"type": kind, "description": d})
        };
        properties.insert((*key).into(), prop);
        if *req {
            required.push(*key);
        }
    }
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": desc,
            "parameters": {
                "type": "object",
                "properties": properties,
                "required": required,
            }
        }
    })
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
        properties.insert((*key).into(), json!({"type": "string", "description": d}));
        if *req {
            required.push(*key);
        }
    }
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": desc,
            "parameters": {
                "type": "object",
                "properties": properties,
                "required": required,
            }
        }
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
        let schemas = tools.iter().map(|t| t.schema()).collect();
        View {
            tools: Arc::new(tools),
            schemas: Arc::new(schemas),
        }
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
mod goals_guard_tests {
    use super::*;

    #[test]
    fn a_child_may_not_write_goals_md_but_root_may() {
        let dir = std::env::temp_dir().join(format!("arbos-goals-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".arbos")).unwrap();
        std::fs::write(dir.join(".arbos/GOALS.md"), "# g\n").unwrap();
        let root = arbos_core::Agent::root("root");
        let mut child = arbos_core::Agent::root("kid");
        child.parent = Some(arbos_core::AgentId::new("root"));
        let for_root = PlanCx {
            root: &dir,
            cwd: &dir,
            agent: &root,
        };
        assert!(for_root.resolve_write(".arbos/GOALS.md").is_ok());
        let for_child = PlanCx {
            root: &dir,
            cwd: &dir,
            agent: &child,
        };
        let err = for_child.resolve_write(".arbos/GOALS.md").unwrap_err();
        assert!(err.to_string().contains("owned by the main chat"), "{err}");
        assert!(for_child.resolve_write("main.py").is_ok());
        std::fs::write(dir.join(".arbos/notes.md"), "# notes\n").unwrap();
        assert!(for_root.resolve_write(".arbos/notes.md").is_ok());
        let err = for_child.resolve_write(".arbos/notes.md").unwrap_err();
        assert!(err.to_string().contains("project page"), "{err}");
    }
}
