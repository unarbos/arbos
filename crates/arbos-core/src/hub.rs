//! The mesh: every Arbos Jacob runs can reach every other one.
//!
//! One small service, `arbos-hub`, sits at a public address. Kernels and
//! worker daemons connect to it *outbound* over a WebSocket and register a
//! machine name; nothing on a laptop needs an open port. A client (the
//! desktop, the phone, another kernel) attaches to a kernel by machine
//! name, or claims a worker so it starts a kernel in an existing checkout
//! on that machine. This is Cursor's self-hosted-worker model: the worker
//! dials out, the service routes work to it by name and labels.
//!
//! This module holds what the hub, the kernel, and the worker share: the
//! wire ([`HubFrame`]), the roster ([`MachineInfo`]), the client config
//! (`~/.config/arbos/hub.toml`), and the on-disk mirror of the roster
//! (`.arbos/machines/`) that lets an agent discover other machines with
//! `ls`, per the file-system principle.

use crate::Place;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// What the hub speaks. Bumped when a frame changes meaning.
pub const HUB_PROTOCOL: u32 = 1;

/// Env var carrying the machine token for the hub, when `hub.toml` names
/// no `token`.
pub const TOKEN_ENV: &str = "ARBOS_HUB_TOKEN";

/// A registrant is a kernel (serves one project) or a worker (starts
/// kernels on demand for the checkouts under its directory).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RegistrantKind {
    Kernel,
    Worker,
}

impl RegistrantKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RegistrantKind::Kernel => "kernel",
            RegistrantKind::Worker => "worker",
        }
    }
}

/// Frames between the hub and anything connected to it. The attach wire
/// (`wire::Frame`) rides inside `frame` on a numbered channel as raw
/// JSON, so one registration socket carries any number of attached
/// clients and the hub needs no knowledge of the kernel's frames.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum HubFrame {
    /// Registrant → hub, first frame after the socket opens.
    Register {
        machine: String,
        kind: RegistrantKind,
        #[serde(default)]
        user: String,
        #[serde(default)]
        host: String,
        /// Kernel: the project it serves. Worker: absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        place: Option<String>,
        /// Worker: checkouts it can start a kernel in.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        projects: Vec<String>,
        /// The face of each project named (the kernel's own, a worker's
        /// checkouts), by project name, from each `project.toml`.
        #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        identities: std::collections::BTreeMap<String, crate::project::ProjectIdentity>,
        /// Each named project's sharing mode (`[share] mode` in its
        /// `project.toml`), by project name; absent = `mesh`.
        #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        shares: std::collections::BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        labels: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        capabilities: Vec<String>,
        #[serde(default)]
        version: String,
        /// The registrant's build: short git sha and build time, beside
        /// `version` (which sits still for weeks). `GET /list` shows them
        /// so "which of my machines is stale" is one request.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        git_sha: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        build: String,
        protocol: u32,
    },
    /// Hub → registrant: accepted under this name.
    Registered {
        machine: String,
        id: u64,
    },
    /// Hub → every registrant, on every change: the machines it knows.
    Roster {
        machines: Vec<MachineInfo>,
    },
    /// Hub → kernel: a client attached; `chan` names it from now on. `who`
    /// and `role` were verified by the hub.
    Open {
        chan: u64,
        who: String,
        role: String,
    },
    /// Both ways: one attach frame for channel `chan`, carried as the
    /// JSON it was sent as. The hub never parses it into a typed `Frame`:
    /// a hub older than the kernel would drop the fields it did not know
    /// (a `put` lost its `data`, a `history` its `before`, silently). Only
    /// the two ends — the client and the kernel, each on its own version —
    /// read it; the hub checks it is an object with a `type` and passes
    /// it through whole.
    Frame {
        chan: u64,
        frame: serde_json::Value,
    },
    /// Both ways: channel `chan` is gone.
    Close {
        chan: u64,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        reason: String,
    },
    /// Client → hub → worker: start a kernel for `project` on the machine.
    /// `isolate` asks for a git worktree of the checkout, so the claimer
    /// never edits the machine's own working copy.
    Claim {
        #[serde(default)]
        id: String,
        project: String,
        #[serde(default)]
        isolate: bool,
        #[serde(default)]
        from: String,
    },
    /// Worker → hub → client: the outcome. `project` is the name the new
    /// kernel registered under (a worktree gets `<project>--<id>`).
    Claimed {
        id: String,
        machine: String,
        project: String,
        #[serde(default)]
        place: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        detail: String,
    },
    Error {
        detail: String,
    },
    /// Kernel → hub: a notification the user should hear about even with
    /// no client attached (the same as the attach protocol's `notify`),
    /// so the hub can push it to a phone that is asleep. `unseen` is the
    /// count after this one, for the badge.
    Notify {
        project: String,
        id: u64,
        ts: i64,
        agent: String,
        kind: String,
        title: String,
        body: String,
        #[serde(default)]
        unseen: u64,
    },
    /// Kernel → hub: the user has seen through `through`; `unseen` is what
    /// is left, so a phone's badge drops without the app running.
    Seen {
        project: String,
        through: u64,
        #[serde(default)]
        unseen: u64,
    },
    /// A frame this build does not know. Skipped, never fatal.
    #[serde(other)]
    Unknown,
}

/// One project a machine can serve: a live kernel, or a checkout a
/// worker can start one in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub place: String,
    /// A kernel serves it now.
    #[serde(default)]
    pub live: bool,
    /// Where this project's store is, as every node addresses it:
    /// `arbos://<machine>/<project>/`. Filled by the hub. See
    /// [`StoreAddress`].
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub store: String,
    /// The project's sharing mode from its `project.toml` (`[share] mode`):
    /// `private`, `mesh` (the default), or `open`. See [`store_access`].
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub share: String,
    /// What the recipient of this roster may do in that store: `owner`,
    /// `writer`, `reader`, or `none`. The hub computes it for each
    /// recipient from its token and the project's `share`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub access: String,
    /// The project's face from its `project.toml` (name, glyph, colour),
    /// as the registering kernel or worker read it; absent when the
    /// folder has no file. A phone draws its list from this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<crate::project::ProjectIdentity>,
    /// `worktree` when this is a worker's git worktree of another project
    /// on the machine (a claim with `isolate`), not a project of the
    /// user's; absent for a project. A client nests or hides it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// For a worktree: the project it was cut from (`demo` for
    /// `demo--c616190-1`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

/// One machine as the hub sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineInfo {
    pub name: String,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub version: String,
    /// The newest registrant's build (short git sha, build time).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub git_sha: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub build: String,
    /// A worker daemon is connected: `spawn host=<name>` can claim it.
    #[serde(default)]
    pub worker: bool,
    #[serde(default)]
    pub projects: Vec<ProjectInfo>,
    /// Unix millis of the first registration still connected.
    #[serde(default)]
    pub since: i64,
}

impl MachineInfo {
    /// The stores on this machine the roster's recipient may read:
    /// `(address, access)` pairs, live kernels first.
    pub fn readable_stores(&self) -> Vec<(String, String)> {
        self.projects
            .iter()
            .filter(|p| {
                !p.store.is_empty() && matches!(p.access.as_str(), "owner" | "writer" | "reader")
            })
            .map(|p| (p.store.clone(), p.access.clone()))
            .collect()
    }

    /// One roster line for the prompt and `machines.md`.
    pub fn describe(&self) -> String {
        let mut s = self.name.clone();
        let mut bits: Vec<String> = Vec::new();
        if self.worker {
            bits.push("worker".into());
        }
        let live: Vec<&str> = self
            .projects
            .iter()
            .filter(|p| p.live)
            .map(|p| p.name.as_str())
            .collect();
        if !live.is_empty() {
            bits.push(format!("kernels: {}", live.join(", ")));
        }
        let idle: Vec<&str> = self
            .projects
            .iter()
            .filter(|p| !p.live)
            .map(|p| p.name.as_str())
            .collect();
        if !idle.is_empty() {
            bits.push(format!("checkouts: {}", idle.join(", ")));
        }
        let mut tags = self.labels.clone();
        tags.extend(self.capabilities.iter().cloned());
        if !tags.is_empty() {
            bits.push(tags.join(", "));
        }
        if !bits.is_empty() {
            s.push_str(" (");
            s.push_str(&bits.join("; "));
            s.push(')');
        }
        s
    }
}

/// `~/.config/arbos/hub.toml`: which hub this machine dials, as whom.
///
/// ```toml
/// url = "wss://hub-api.arbos.life"
/// machine = "arboslife"
/// token_env = "ARBOS_HUB_TOKEN"   # or token = "…" (file mode 0600)
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct HubConfig {
    pub url: String,
    pub machine: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_env: Option<String>,
}

impl HubConfig {
    pub fn path() -> PathBuf {
        crate::host_dir().join("hub.toml")
    }

    /// The file, or `None` when this machine has no hub configured. A
    /// malformed file is an error: it is the user's file.
    pub fn load() -> Result<Option<Self>> {
        let path = Self::path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Ok(None);
        };
        let cfg: Self =
            toml::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
        if cfg.url.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(cfg))
    }

    /// The file, with `--hub` and `--machine` flags (as env) on top.
    pub fn resolve(url: Option<&str>, machine: Option<&str>) -> Result<Option<Self>> {
        let mut cfg = Self::load()?.unwrap_or_default();
        if let Some(u) = url.map(str::trim).filter(|u| !u.is_empty()) {
            cfg.url = u.to_string();
        }
        if let Some(m) = machine.map(str::trim).filter(|m| !m.is_empty()) {
            cfg.machine = m.to_string();
        }
        if cfg.url.is_empty() {
            return Ok(None);
        }
        if cfg.machine.is_empty() {
            cfg.machine = default_machine_name();
        }
        Ok(Some(cfg))
    }

    /// The machine token: `token`, else `token_env`, else `ARBOS_HUB_TOKEN`.
    pub fn token(&self) -> Result<String> {
        if let Some(t) = self
            .token
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            return Ok(t.to_string());
        }
        let var = self.token_env.as_deref().unwrap_or(TOKEN_ENV);
        match std::env::var(var) {
            Ok(v) if !v.trim().is_empty() => Ok(v.trim().to_string()),
            _ => bail!(
                "no hub token: set {var} or put token = \"…\" in {}",
                Self::path().display()
            ),
        }
    }

    /// `wss://host/attach/<machine>[/<project>]`.
    pub fn attach_url(&self, machine: &str, project: Option<&str>) -> String {
        let mut u = format!("{}/attach/{machine}", self.url.trim_end_matches('/'));
        if let Some(p) = project.filter(|p| !p.is_empty()) {
            u.push('/');
            u.push_str(p);
        }
        u
    }

    pub fn claim_url(&self, machine: &str) -> String {
        format!("{}/claim/{machine}", self.url.trim_end_matches('/'))
    }

    pub fn register_url(&self) -> String {
        format!("{}/register", self.url.trim_end_matches('/'))
    }
}

/// This machine's short name when none is configured: the hostname's
/// first label, lower-case.
pub fn default_machine_name() -> String {
    let raw = std::fs::read_to_string("/etc/hostname")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_default();
    let name: String = raw
        .trim()
        .split('.')
        .next()
        .unwrap_or("")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    if name.is_empty() {
        "local".into()
    } else {
        name
    }
}

// ── store addresses ─────────────────────────────────────────────────────

/// The scheme of a store address.
pub const STORE_SCHEME: &str = "arbos://";

/// A file or folder in some node's store, named so any node on the same
/// hub can resolve it: `arbos://<machine>/<project>/<path>`.
///
/// `machine` is the hub roster name, `project` the name the kernel
/// registered under (the two names `say to=<machine>/<project>/<agent>`
/// and `hello` already use), and `path` is relative to that place's
/// `.arbos/` (`notes.md`, `docs/project-context.md`, `agents/root/plan.md`;
/// empty for the store root). An address never points into a checkout:
/// the checkout is work and each machine has its own; the store is the
/// context that crosses machines.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StoreAddress {
    pub machine: String,
    pub project: String,
    pub path: String,
}

impl StoreAddress {
    /// The root of `project`'s store on `machine`.
    pub fn root(machine: &str, project: &str) -> Self {
        Self {
            machine: machine.to_string(),
            project: project.to_string(),
            path: String::new(),
        }
    }

    /// Does `s` look like a store address (whatever its validity)?
    pub fn looks_like(s: &str) -> bool {
        s.trim_start().starts_with(STORE_SCHEME)
    }

    /// Parse `arbos://<machine>/<project>[/<path>]`. `..` in the path,
    /// an empty machine or project, and a path into `../` are refused;
    /// the receiving kernel confines again, this is the first gate.
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();
        let rest = s
            .strip_prefix(STORE_SCHEME)
            .with_context(|| format!("{s:?}: a store address starts with {STORE_SCHEME}"))?;
        let mut parts = rest.splitn(3, '/');
        let machine = parts.next().unwrap_or("").trim();
        let project = parts.next().unwrap_or("").trim();
        let path = parts.next().unwrap_or("").trim().trim_start_matches('/');
        if machine.is_empty() || project.is_empty() {
            bail!("{s:?}: a store address is {STORE_SCHEME}<machine>/<project>/<path>");
        }
        let bad = |c: char| c == '/' || c == ' ' || c == ':' || c == '@';
        if machine.contains(bad) || project.contains(bad) {
            bail!("{s:?}: machine and project names hold no space, slash, colon, or @");
        }
        if path.split('/').any(|seg| seg == "..") {
            bail!("{s:?}: a store path does not climb out with ..");
        }
        let path = path.strip_prefix(".arbos/").unwrap_or(path);
        Ok(Self {
            machine: machine.to_string(),
            project: project.to_string(),
            path: path.trim_end_matches('/').to_string(),
        })
    }

    /// The address of `rel` under this address.
    pub fn join(&self, rel: &str) -> Self {
        let rel = rel.trim().trim_start_matches("./").trim_matches('/');
        let path = if self.path.is_empty() {
            rel.to_string()
        } else if rel.is_empty() {
            self.path.clone()
        } else {
            format!("{}/{rel}", self.path)
        };
        Self {
            machine: self.machine.clone(),
            project: self.project.clone(),
            path,
        }
    }

    /// Is this address on the node named `machine` serving `project`?
    /// Then it is a local file, and the fast path applies.
    pub fn is_node(&self, machine: &str, project: &str) -> bool {
        self.machine.eq_ignore_ascii_case(machine) && self.project == project
    }

    /// The local file for an address on this very node: `<arbos>/<path>`.
    pub fn local_path(&self, place: &Place) -> PathBuf {
        if self.path.is_empty() {
            place.arbos()
        } else {
            place.arbos().join(&self.path)
        }
    }
}

impl std::fmt::Display for StoreAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{STORE_SCHEME}{}/{}/{}",
            self.machine, self.project, self.path
        )
    }
}

/// The sha-256 of `bytes` as lower-case hex: the `hash` of a `written`
/// frame and the `base_hash` of a `put`. Stable across machines and
/// builds, so a writer's view and the store's can be compared by value.
pub fn content_hash(bytes: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// The names a brief uses for the project store (the contract and the
/// kickoff defaults speak of `docs/`, `internal/`, `media/`, `notes.md`
/// beside `.arbos/…`).
const STORE_WORDS: &[&str] = &[
    ".arbos/",
    "docs/",
    "internal/",
    "media/",
    "notes.md",
    "archived.md",
    "GOALS.md",
];

/// Rewrite the store paths in a brief to addresses on `store`, so a
/// child on another machine reads the parent's Project memory and
/// delivers into the parent's store rather than into its own fresh
/// worktree's. Every token that names the store (`.arbos/docs/x.md`,
/// `docs/x.md`, `notes.md`, a markdown link's target) becomes
/// `arbos://<machine>/<project>/<path>`; code paths and prose stay as they
/// are, and a token that is already an address is left alone.
pub fn address_brief(brief: &str, store: &StoreAddress) -> String {
    let root = StoreAddress::root(&store.machine, &store.project);
    let rewrite_path = |raw: &str| -> Option<String> {
        if raw.starts_with(STORE_SCHEME) {
            return None;
        }
        let trimmed = raw.strip_prefix("./").unwrap_or(raw);
        // A folder counts with its slash (`docs/x`, never the word `docs`);
        // `.arbos` alone is the store itself.
        let names_store = trimmed == ".arbos" || STORE_WORDS.iter().any(|w| trimmed.starts_with(w));
        if !names_store {
            return None;
        }
        let rel = trimmed.strip_prefix(".arbos/").unwrap_or(trimmed);
        let rel = rel.strip_prefix(".arbos").unwrap_or(rel);
        let mut out = root.join(rel).to_string();
        // A folder keeps its trailing slash: `docs/` stays a folder.
        if rel.ends_with('/') && !out.ends_with('/') {
            out.push('/');
        }
        Some(out)
    };
    let mut out = String::with_capacity(brief.len() + 64);
    for line in brief.split_inclusive('\n') {
        let (body, nl) = match line.strip_suffix('\n') {
            Some(b) => (b, "\n"),
            None => (line, ""),
        };
        let mut first = true;
        for raw in body.split(' ') {
            if !first {
                out.push(' ');
            }
            first = false;
            out.push_str(&rewrite_token(raw, &rewrite_path));
        }
        out.push_str(nl);
    }
    out
}

/// One whitespace-delimited token: the path inside its punctuation
/// (backticks, quotes, brackets, a trailing comma or full stop) or after
/// a markdown link's `](`, rewritten when it names the store.
fn rewrite_token(raw: &str, rewrite_path: &dyn Fn(&str) -> Option<String>) -> String {
    const OPEN: &[char] = &['`', '"', '\'', '(', '[', '<', '*', '_'];
    const CLOSE: &[char] = &['`', '"', '\'', ')', ']', '>', ',', '.', ';', ':', '*', '_'];
    if let Some(i) = raw.find("](") {
        // `[label](target)…`: the target is the path.
        let (head, rest) = raw.split_at(i + 2);
        let end = rest.find(')').unwrap_or(rest.len());
        let (target, tail) = rest.split_at(end);
        if let Some(new) = rewrite_path(target) {
            return format!("{head}{new}{tail}");
        }
        return raw.to_string();
    }
    let start = raw.len() - raw.trim_start_matches(OPEN).len();
    let core = &raw[start..];
    let end = core.trim_end_matches(CLOSE).len();
    let (path, close) = core.split_at(end);
    // A trailing slash is part of a folder path, not punctuation.
    match rewrite_path(path) {
        Some(new) => format!("{}{new}{close}", &raw[..start]),
        None => raw.to_string(),
    }
}

/// Sharing modes a project may set in `project.toml` `[share] mode`.
pub const SHARE_PRIVATE: &str = "private";
pub const SHARE_MESH: &str = "mesh";
pub const SHARE_OPEN: &str = "open";

/// What a viewer may do in a project's store: `min(token role, share
/// mode)`.
///
/// - `private`: identities of the project's owner user keep their role;
///   everyone else gets `none`.
/// - `mesh` (the default, and any unknown word): the viewer's token role.
/// - `open`: the token role, but never below `reader` for anyone the hub
///   admitted.
///
/// `viewer_user` and `owner_user` are the `user` of the hub tokens (the
/// viewer's, and the one the project's machine registered with). Roles
/// are `owner`, `writer`, `reader`. Per-file rules (root-owned pages,
/// protected files) apply on top, at the receiving kernel.
pub fn store_access(
    share: &str,
    viewer_user: &str,
    viewer_role: &str,
    owner_user: &str,
) -> &'static str {
    let role = match viewer_role {
        "owner" => "owner",
        "writer" => "writer",
        "reader" => "reader",
        _ => "none",
    };
    match share {
        SHARE_PRIVATE => {
            if viewer_user == owner_user {
                role
            } else {
                "none"
            }
        }
        SHARE_OPEN => {
            if role == "none" {
                "reader"
            } else {
                role
            }
        }
        _ => role,
    }
}

/// A `to=` target of the form `<machine>/<agent>` or
/// `<machine>/<project>/<agent>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshTarget {
    pub machine: String,
    pub project: Option<String>,
    pub agent: String,
}

impl MeshTarget {
    pub fn parse(to: &str) -> Option<Self> {
        let parts: Vec<&str> = to.trim().split('/').map(str::trim).collect();
        match parts.as_slice() {
            [m, a] if !m.is_empty() && !a.is_empty() => Some(Self {
                machine: m.to_string(),
                project: None,
                agent: a.to_string(),
            }),
            [m, p, a] if !m.is_empty() && !p.is_empty() && !a.is_empty() => Some(Self {
                machine: m.to_string(),
                project: Some(p.to_string()),
                agent: a.to_string(),
            }),
            _ => None,
        }
    }
}

// ── the roster on disk ──────────────────────────────────────────────────

/// `.arbos/machines/`: one TOML file per machine the hub knows.
pub fn machines_dir(place: &Place) -> PathBuf {
    place.arbos().join("machines")
}

/// The file a kernel writes for one machine of the roster.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MachineFile {
    #[serde(flatten)]
    info: MachineInfo,
    hub: String,
    /// Unix millis when this file was written.
    seen: i64,
}

/// Write the roster as `.arbos/machines/<name>.toml` plus a rendered
/// `.arbos/machines.md`. Files of machines no longer listed are removed:
/// the folder is a mirror of the hub, not a history.
pub fn write_roster(place: &Place, hub_url: &str, machines: &[MachineInfo]) -> Result<()> {
    let dir = machines_dir(place);
    std::fs::create_dir_all(&dir)?;
    let now = crate::now_ms();
    let mut keep = std::collections::HashSet::new();
    for m in machines {
        let file = MachineFile {
            info: m.clone(),
            hub: hub_url.to_string(),
            seen: now,
        };
        let name = format!("{}.toml", m.name);
        keep.insert(name.clone());
        let path = dir.join(&name);
        let tmp = dir.join(format!(".{name}.tmp"));
        std::fs::write(&tmp, toml::to_string_pretty(&file)?)?;
        std::fs::rename(&tmp, &path)?;
    }
    for entry in std::fs::read_dir(&dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".toml") && !keep.contains(&name) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let mut md = String::from("# Machines on the hub\n\n");
    md.push_str(&format!("Hub: {hub_url}\n\n"));
    if machines.is_empty() {
        md.push_str("(none registered)\n");
    }
    for m in machines {
        md.push_str("- ");
        md.push_str(&m.describe());
        md.push('\n');
        for (store, access) in m.readable_stores() {
            md.push_str(&format!("  - store {store} ({access})\n"));
        }
    }
    md.push_str("\n`spawn host=<name>` runs a child on a machine with a worker, in a worktree of its checkout of this project. `say to=<name>/<agent>` (or `<name>/<project>/<agent>`) messages an agent there. A store address `arbos://<machine>/<project>/<path>` names a file in that node's .arbos/ (notes.md, docs/…, internal/…, media/…, agents/<id>/…); `read`/`ls` take one. Your own store is the plain path.\n");
    let md_path = place.arbos().join("machines.md");
    let tmp = place.arbos().join(".machines.md.tmp");
    std::fs::write(&tmp, md)?;
    std::fs::rename(&tmp, &md_path)?;
    Ok(())
}

/// The roster as last written, oldest file first by name.
pub fn read_roster(place: &Place) -> Vec<MachineInfo> {
    let dir = machines_dir(place);
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<MachineInfo> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|t| toml::from_str::<MachineFile>(&t).ok())
        .map(|f| f.info)
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// One machine from the on-disk roster, by name (case-insensitive).
pub fn roster_machine(place: &Place, name: &str) -> Option<MachineInfo> {
    read_roster(place)
        .into_iter()
        .find(|m| m.name.eq_ignore_ascii_case(name.trim()))
}

/// The hub roster as one prompt line, or `None` when the folder is empty.
pub fn roster_line(place: &Place) -> Option<String> {
    let machines = read_roster(place);
    if machines.is_empty() {
        return None;
    }
    let stores = machines.iter().flat_map(|m| m.readable_stores()).count();
    Some(format!(
        "Hub machines (spawn host=<name>, say to=<name>/<agent>; details in .arbos/machines/): {}{}",
        machines
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        if stores > 0 {
            format!(
                ". Their stores read by address (arbos://<machine>/<project>/<path>, listed in .arbos/machines.md): {stores}"
            )
        } else {
            String::new()
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_target_parses_two_and_three_parts() {
        assert_eq!(
            MeshTarget::parse("mac/root"),
            Some(MeshTarget {
                machine: "mac".into(),
                project: None,
                agent: "root".into()
            })
        );
        assert_eq!(
            MeshTarget::parse("mac/arbos/fix-build"),
            Some(MeshTarget {
                machine: "mac".into(),
                project: Some("arbos".into()),
                agent: "fix-build".into()
            })
        );
        assert_eq!(MeshTarget::parse("root"), None);
        assert_eq!(MeshTarget::parse("a//b"), None);
    }

    #[test]
    fn a_store_address_names_machine_project_and_a_path_in_the_store() {
        let a = StoreAddress::parse("arbos://cloud/demo/docs/project-context.md").unwrap();
        assert_eq!(
            a,
            StoreAddress {
                machine: "cloud".into(),
                project: "demo".into(),
                path: "docs/project-context.md".into()
            }
        );
        assert_eq!(a.to_string(), "arbos://cloud/demo/docs/project-context.md");
        // The root, with and without the trailing slash; `.arbos/` folded.
        let root = StoreAddress::parse("arbos://arboslife/demo--c1").unwrap();
        assert_eq!(root, StoreAddress::root("arboslife", "demo--c1"));
        assert_eq!(root.to_string(), "arbos://arboslife/demo--c1/");
        assert_eq!(
            StoreAddress::parse("arbos://arboslife/demo/").unwrap().path,
            ""
        );
        assert_eq!(
            StoreAddress::parse("arbos://cloud/demo/.arbos/notes.md")
                .unwrap()
                .path,
            "notes.md"
        );
        assert_eq!(
            root.join("agents/root/plan.md").to_string(),
            "arbos://arboslife/demo--c1/agents/root/plan.md"
        );
        assert_eq!(a.join("").path, "docs/project-context.md");
        // The same node's address is the local file.
        assert!(a.is_node("Cloud", "demo"));
        assert!(!a.is_node("cloud", "other"));
        let place = Place::new(std::path::PathBuf::from("/p/demo"));
        assert_eq!(
            a.local_path(&place),
            std::path::PathBuf::from("/p/demo/.arbos/docs/project-context.md")
        );
        assert_eq!(
            root.local_path(&place),
            std::path::PathBuf::from("/p/demo/.arbos")
        );
        // Refusals.
        for bad in [
            "docs/x.md",
            "arbos://cloud",
            "arbos:///demo/x",
            "arbos://cloud/demo/../secret",
            "arbos://a b/demo/x",
        ] {
            assert!(StoreAddress::parse(bad).is_err(), "{bad}");
        }
        assert!(StoreAddress::looks_like("  arbos://x/y/z"));
        assert!(!StoreAddress::looks_like("/tmp/arbos://x"));
    }

    /// A kickoff brief carries paths, not content. For a child on another
    /// machine every store path becomes the parent's address, so the
    /// child reads the Project's memory and delivers into the Project;
    /// code paths and prose are untouched.
    #[test]
    fn a_brief_for_a_remote_child_names_the_parents_store_by_address() {
        let store = StoreAddress::root("cloud", "demo");
        let brief = "Read first: .arbos/docs/project-context.md, then .arbos/notes.md\n\
Task: Fix the echo gate in src/gateway.rs; see `docs/echo.md` and (internal/audit.md).\n\
Output: Deliverables under .arbos/docs/, working notes under .arbos/internal/, captures under .arbos/media/<topic>/. Verify each file exists before you report it.\n\
Report: link [the audit](docs/echo.md); read notes.md; keep ./media/mesh/1.txt and GOALS.md; arbos://mac/x/docs/a.md stays.\n";
        let out = address_brief(brief, &store);
        let want = "Read first: arbos://cloud/demo/docs/project-context.md, then arbos://cloud/demo/notes.md\n\
Task: Fix the echo gate in src/gateway.rs; see `arbos://cloud/demo/docs/echo.md` and (arbos://cloud/demo/internal/audit.md).\n\
Output: Deliverables under arbos://cloud/demo/docs/, working notes under arbos://cloud/demo/internal/, captures under arbos://cloud/demo/media/<topic>/. Verify each file exists before you report it.\n\
Report: link [the audit](arbos://cloud/demo/docs/echo.md); read arbos://cloud/demo/notes.md; keep arbos://cloud/demo/media/mesh/1.txt and arbos://cloud/demo/GOALS.md; arbos://mac/x/docs/a.md stays.\n";
        assert_eq!(out, want);
        // Words that only contain a store word are prose, not paths.
        assert_eq!(
            address_brief("the docs are in mydocs/x", &store),
            "the docs are in mydocs/x"
        );
        assert_eq!(address_brief(".arbos", &store), "arbos://cloud/demo/");
    }

    #[test]
    fn store_access_is_the_token_role_capped_by_the_share_mode() {
        // mesh (default, and any unknown word): the token's role.
        assert_eq!(store_access("mesh", "alice", "writer", "owner"), "writer");
        assert_eq!(store_access("", "owner", "owner", "owner"), "owner");
        assert_eq!(store_access("weird", "bob", "reader", "owner"), "reader");
        assert_eq!(store_access("mesh", "bob", "bogus", "owner"), "none");
        // private: only the owner user's identities, at their role.
        assert_eq!(store_access("private", "owner", "owner", "owner"), "owner");
        assert_eq!(
            store_access("private", "owner", "reader", "owner"),
            "reader"
        );
        assert_eq!(store_access("private", "alice", "owner", "owner"), "none");
        // open: never below reader for anyone admitted.
        assert_eq!(store_access("open", "alice", "bogus", "owner"), "reader");
        assert_eq!(store_access("open", "alice", "writer", "owner"), "writer");
    }

    /// The channel frame carries the attach frame as JSON, so a field a
    /// build does not know rides through it untouched.
    #[test]
    fn a_channel_frame_keeps_fields_this_build_does_not_know() {
        let line = r#"{"type":"frame","chan":3,"frame":{"type":"put","path":"a","data":"QUJD","later":true}}"#;
        let f: HubFrame = serde_json::from_str(line).unwrap();
        let HubFrame::Frame { chan, frame } = &f else {
            panic!("{f:?}");
        };
        assert_eq!(*chan, 3);
        assert_eq!(frame["data"], "QUJD");
        assert_eq!(frame["later"], true);
        let back = serde_json::to_string(&f).unwrap();
        assert!(back.contains(r#""later":true"#), "{back}");
    }

    #[test]
    fn hub_frame_unknown_is_skipped_not_fatal() {
        let f: HubFrame = serde_json::from_str(r#"{"type":"later_thing","x":1}"#).unwrap();
        assert!(matches!(f, HubFrame::Unknown));
    }

    #[test]
    fn roster_round_trips_through_the_folder() {
        let dir = std::env::temp_dir().join(format!("arbos-hub-roster-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".arbos")).unwrap();
        let place = Place::new(dir.clone());
        let m = MachineInfo {
            name: "arboslife".into(),
            user: "const".into(),
            host: "box".into(),
            labels: vec!["linux".into()],
            capabilities: vec!["gpu".into()],
            version: "0.2.0".into(),
            git_sha: "abc123def456".into(),
            build: "2026-09-16T11:55Z".into(),
            worker: true,
            projects: vec![ProjectInfo {
                identity: Some(crate::project::ProjectIdentity {
                    name: Some("Demo".into()),
                    icon: "terminal".into(),
                    color: "teal".into(),
                }),
                name: "demo".into(),
                place: "/x/demo".into(),
                live: false,
                store: "arbos://arboslife/demo/".into(),
                share: "mesh".into(),
                access: "owner".into(),
                kind: String::new(),
                parent: None,
            }],
            since: 1,
        };
        write_roster(&place, "wss://hub", &[m.clone()]).unwrap();
        assert_eq!(read_roster(&place), vec![m.clone()]);
        // The store's address and the reader's rights are on disk and in
        // the rendered page, so `ls .arbos/machines/` answers "what stores
        // exist on my peers and which may I touch".
        let file = std::fs::read_to_string(machines_dir(&place).join("arboslife.toml")).unwrap();
        assert!(
            file.contains("store = \"arbos://arboslife/demo/\""),
            "{file}"
        );
        assert!(file.contains("access = \"owner\""), "{file}");
        let md = std::fs::read_to_string(place.arbos().join("machines.md")).unwrap();
        assert!(
            md.contains("- store arbos://arboslife/demo/ (owner)"),
            "{md}"
        );
        assert_eq!(
            m.readable_stores(),
            vec![("arbos://arboslife/demo/".to_string(), "owner".to_string())]
        );
        let mut none = m.clone();
        none.projects[0].access = "none".into();
        assert!(none.readable_stores().is_empty());
        // An old roster without the fields still reads.
        let old: ProjectInfo = serde_json::from_str(r#"{"name":"demo","live":true}"#).unwrap();
        assert!(old.store.is_empty() && old.access.is_empty());
        assert!(
            roster_line(&place)
                .unwrap()
                .contains("arbos://<machine>/<project>/<path>")
        );
        // The face rides in the roster as the phone reads it.
        let json = serde_json::to_value(&m.projects[0]).unwrap();
        assert_eq!(json["identity"]["icon"], "terminal");
        assert_eq!(json["identity"]["name"], "Demo");
        // An older hub without the field still reads.
        let old: ProjectInfo = serde_json::from_str(r#"{"name":"demo","live":true}"#).unwrap();
        assert!(old.identity.is_none());
        // Prompt-size pass (2026-09-13): the roster in the prompt carries
        // names only; the details stay in .arbos/machines/ for `read`.
        let line = roster_line(&place).unwrap();
        assert!(line.contains("arboslife"), "{line}");
        assert!(line.contains(".arbos/machines/"), "{line}");
        assert!(!line.contains("checkouts"), "{line}");
        write_roster(&place, "wss://hub", &[]).unwrap();
        assert!(read_roster(&place).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
