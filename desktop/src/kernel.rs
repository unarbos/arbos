//! Attach to an Arbos kernel, or start one, in a workspace directory.
//!
//! The kernel is a detached `arbos-kernel serve` process.
//! Local workspaces read `<workspace>/.arbos/kernel.json`. Remote ones probe
//! over ssh, start the kernel on the host if needed, and talk through an
//! `ssh -N -L` tunnel on loopback. Quitting the window leaves the kernel
//! running, and tears down every tunnel this process opened.

use crate::model::{place::Place, session, settings};
use anyhow::{Context, Result, anyhow};
use arbos_core::host::{Host, HostConfig, KeySource, ProviderKind, attribution_headers};
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, OnceLock},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Contents of `<workspace>/.arbos/kernel.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct WebInfo {
    pub url: String,
    pub pid: i32,
    /// Older kernels omit this. Missing must not make the file unreadable —
    /// that is the 60s timeout on a port nobody opened.
    #[serde(default)]
    #[allow(dead_code)]
    pub started: i64,
}

const READY_WAIT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(200);
/// Where `arbos-kernel` lives on a host that `machines.toml` does not
/// describe. A described machine says itself (`kernel`, default
/// `<dir>/bin/arbos-kernel`).
const REMOTE_BIN: &str = "$HOME/.cargo/bin/arbos-kernel";

/// The attach protocol this window speaks; a kernel that says less is
/// refused (`arbos_kernel::serve::PROTOCOL` on the other side).
pub const PROTOCOL: u32 = 1;

/// One ssh host as this window reaches it: the ssh target, the kernel
/// binary's path there, the config home its kernels read, and whether a
/// source build is allowed when no binary can be copied. From
/// `~/.config/arbos/machines.toml` when the host is a machine there (by
/// name or by `ssh` target) — the same fields `spawn host=` uses — else the
/// old defaults.
#[derive(Debug, Clone)]
pub struct RemoteTarget {
    pub ssh: String,
    pub bin: String,
    pub config_home: Option<String>,
    pub build: bool,
    pub name: String,
}

pub fn remote_target(host: &str) -> RemoteTarget {
    if let Ok(machines) = arbos_core::Machines::load()
        && let Some(m) = machines.get(host)
    {
        return RemoteTarget {
            ssh: m.target().to_string(),
            bin: m.kernel_path(),
            config_home: Some(m.config_home()),
            build: m.build,
            name: m.name.clone(),
        };
    }
    RemoteTarget {
        ssh: host.to_string(),
        bin: REMOTE_BIN.to_string(),
        config_home: None,
        build: false,
        name: host.to_string(),
    }
}
const REMOTE_PORTS: (u16, u16) = (20000, 32000);

#[cfg(test)]
mod tests;

struct Tunnel {
    child: Child,
    info: WebInfo,
    /// Loopback HTTP origin for this place's gateway (`web.json`), when the
    /// remote kernel advertises one. Attach stays on `info` (`kernel.json` TCP).
    http: Option<String>,
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn tunnels() -> &'static Mutex<HashMap<String, Tunnel>> {
    static TUNNELS: OnceLock<Mutex<HashMap<String, Tunnel>>> = OnceLock::new();
    TUNNELS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn tunnels_lock() -> std::sync::MutexGuard<'static, HashMap<String, Tunnel>> {
    tunnels()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn mux_hosts() -> &'static Mutex<HashSet<String>> {
    static HOSTS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    HOSTS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn mux_hosts_lock() -> std::sync::MutexGuard<'static, HashSet<String>> {
    mux_hosts()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Kill every `ssh -N -L` this process opened, and exit muxes it created.
/// The remote kernel stays up.
pub fn shutdown_tunnels() {
    let taken: Vec<Tunnel> = {
        let mut map = tunnels_lock();
        map.drain().map(|(_, tunnel)| tunnel).collect()
    };
    drop(taken);
    let hosts: Vec<String> = {
        let mut hosts = mux_hosts_lock();
        hosts.drain().collect()
    };
    let dir = ssh_control_dir();
    let control = format!("ControlPath={}/{}", dir.display(), "%C");
    for host in hosts {
        let _ = Command::new("ssh")
            .args(ssh_base())
            .args([
                "-o",
                "ControlMaster=auto",
                "-o",
                &control,
                "-O",
                "exit",
                &host,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn remember_mux_host(host: &str) {
    mux_hosts_lock().insert(host.to_owned());
}

/// Process-stack guard: Drop of a static map is not reliable on exit.
pub struct TunnelGuard;

impl Drop for TunnelGuard {
    fn drop(&mut self) {
        shutdown_tunnels();
    }
}

pub fn install_shutdown() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        #[cfg(unix)]
        unsafe {
            extern "C" fn on_exit() {
                shutdown_tunnels();
            }
            libc_atexit(on_exit);
        }
    });
}

#[cfg(unix)]
unsafe extern "C" {
    #[link_name = "atexit"]
    fn libc_atexit(cb: extern "C" fn()) -> i32;
}

/// Shared HTTP client. Every probe used to hang forever: ureq has no default
/// timeout, and a wedged kernel then wedged the UI thread that asked.
fn http() -> ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT
        .get_or_init(|| {
            ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(2)))
                .build()
                .into()
        })
        .clone()
}

/// Find a live kernel for `place`, or start one.
pub fn attach_or_spawn_place(place: &Place) -> Result<WebInfo> {
    match &place.host {
        None => attach_or_spawn(&place.path),
        Some(host) => attach_remote(host, &place.path),
    }
}

/// Find a live kernel for `workspace`, or start one.
pub fn attach_or_spawn(workspace: &Path) -> Result<WebInfo> {
    let workspace = workspace
        .canonicalize()
        .with_context(|| format!("workspace is not a directory: {}", workspace.display()))?;
    // The live kernel bootstraps only at start. A later delete (or a
    // missing tree) can leave `.arbos/agents/root` gone while the
    // socket is still up — every new chat attaches as `root`, so
    // recreate the folder here.
    let _ = arbos_core::bootstrap(&arbos_core::Place::new(&workspace));
    if let Some(info) = read_info(&workspace).filter(alive) {
        return Ok(info);
    }
    let child = spawn(&workspace)?;
    wait_ready(&workspace, child)
}

pub fn websocket_url(info: &WebInfo) -> String {
    let base = info
        .url
        .trim_end_matches('/')
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);
    format!("{base}/api/ws")
}

/// Loopback HTTP origin for a live kernel, if one answers.
/// Local reads that place's `web.json`. Remote uses the tunnelled gateway
/// from the remote place's `web.json` — never the attach `tcp://` URL.
pub fn http_base_place(place: &Place) -> Option<String> {
    match &place.host {
        None => http_base(&place.path),
        Some(_) => {
            let key = place.encode();
            let (info, http) = {
                let map = tunnels_lock();
                let tunnel = map.get(&key)?;
                (tunnel.info.clone(), tunnel.http.clone())
            };
            alive(&info).then_some(http).flatten()
        }
    }
}

/// Loopback HTTP origin for the gateway (`GET /api/models`, sessions).
/// Attach is `tcp://` in `kernel.json`. HTTP is `web.json`.
pub fn http_base(workspace: &Path) -> Option<String> {
    // Only an HTTP address is worth a probe. `kernel.json` is `tcp://` on
    // every kernel of this generation, and probing it opened and closed an
    // attach socket — logged by the kernel as a client — on every poll.
    let gateway = read_json_info(&gateway_json(workspace))
        .filter(|info| http_url(info).is_some())
        .filter(alive);
    if let Some(url) = gateway.as_ref().and_then(http_url) {
        return Some(url);
    }
    read_info(workspace)
        .filter(|info| http_url(info).is_some())
        .filter(alive)
        .and_then(|info| http_url(&info))
}

/// One piece of machine work that belongs to a chat: a sub-agent or a
/// scheduled firing that is in flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveWork {
    pub label: String,
    pub running: bool,
}

/// Slash skills and prompt templates for `place`. Kernel `/api/commands`
/// first (built-ins live in that binary), then local skill and prompt
/// files so a stale kernel still lists what is on disk.
pub fn list_commands(place: &Place) -> Vec<session::Command> {
    let mut out = Vec::new();
    let mut seen = HashMap::new();
    let add = |out: &mut Vec<session::Command>,
               seen: &mut HashMap<String, ()>,
               name: String,
               description: String| {
        let key = name.to_ascii_lowercase();
        if seen.contains_key(&key) || name.is_empty() {
            return;
        }
        seen.insert(key, ());
        out.push(session::Command { name, description });
    };
    if let Some(base) = http_base_place(place) {
        if let Ok(mut resp) = http().get(&format!("{base}/api/commands")).call() {
            if let Ok(body) = resp.body_mut().read_to_string() {
                if let Ok(parsed) = serde_json::from_str::<CommandsBody>(&body) {
                    for row in parsed.commands.unwrap_or_default() {
                        add(
                            &mut out,
                            &mut seen,
                            row.name,
                            row.description.unwrap_or_default(),
                        );
                    }
                }
            }
        }
    }
    if place.host.is_none() {
        for dir in [
            place.path.join(".arbos").join("skills"),
            dirs::home_dir()
                .map(|home| home.join(".config").join("arbos").join("skills"))
                .unwrap_or_default(),
        ] {
            if dir.as_os_str().is_empty() {
                continue;
            }
            for (name, description) in load_skills_dir(&dir, true) {
                add(&mut out, &mut seen, name, description);
            }
        }
        for dir in [
            place.path.join(".arbos").join("prompts"),
            dirs::home_dir()
                .map(|home| home.join(".config").join("arbos").join("prompts"))
                .unwrap_or_default(),
        ] {
            if dir.as_os_str().is_empty() {
                continue;
            }
            for (name, description) in load_prompt_dir(&dir) {
                add(&mut out, &mut seen, name, description);
            }
        }
        // The kernel's own verbs, last so a skill or prompt of the same
        // name wins. The window answers these itself (see `Arbos::submit`).
        for (name, description) in BUILTIN_COMMANDS {
            add(
                &mut out,
                &mut seen,
                (*name).to_string(),
                (*description).to_string(),
            );
        }
    }
    out
}

/// Slash commands every local chat has, answered by the window with a kernel
/// frame rather than sent to the model as text.
pub const BUILTIN_COMMANDS: &[(&str, &str)] = &[
    ("compact", "Summarise the oldest turns now to free context"),
    (
        "undo",
        "Restore the files to how they were when this turn started",
    ),
    ("stop", "Stop the current turn"),
    ("model", "Switch model: /model <id>"),
    ("mode", "Permission mode: /mode auto | ask | plan"),
    ("pause", "Pause this agent: prompts wait until /resume"),
    ("resume", "Resume a paused agent"),
    ("fork", "Copy this chat into a new one"),
];

/// One model the provider will accept. `name` is what the menu shows; `id`
/// is what `set_model` sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOption {
    pub id: String,
    pub name: String,
}

/// The composer's model list, plus the kernel's current selection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelsCatalog {
    pub models: Vec<ModelOption>,
    pub current: String,
    /// Why the list is empty, when the picker should say so. Empty when
    /// the catalog arrived or there is nothing useful to show.
    pub error: String,
}

/// Provider catalog for `place`. OpenRouter (`config.toml` `api_base`)
/// first — that is the turn host. Gateway `/api/models` is the fallback
/// when the host listing is unreachable. Attach (`tcp://` in
/// `kernel.json`) is never the URL.
pub fn list_models(place: &Place) -> ModelsCatalog {
    if let Some(catalog) = fetch_host_models() {
        if !catalog.models.is_empty() {
            return catalog;
        }
    }
    match http_base_place(place) {
        Some(base) => fetch_gateway_models(&base),
        None => ModelsCatalog {
            error: "Gateway offline".into(),
            ..Default::default()
        },
    }
}

fn fetch_gateway_models(base: &str) -> ModelsCatalog {
    let client: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into();
    let Ok(mut resp) = client.get(&format!("{base}/api/models")).call() else {
        return ModelsCatalog {
            error: "Gateway offline".into(),
            ..Default::default()
        };
    };
    let Ok(body) = resp.body_mut().read_to_string() else {
        return ModelsCatalog {
            error: "Gateway offline".into(),
            ..Default::default()
        };
    };
    let Ok(parsed) = serde_json::from_str::<ModelsBody>(&body) else {
        return ModelsCatalog {
            error: "Bad models response".into(),
            ..Default::default()
        };
    };
    let mut models: Vec<ModelOption> = parsed
        .models
        .unwrap_or_default()
        .into_iter()
        .filter(|row| !row.id.is_empty())
        .map(|row| ModelOption {
            name: model_display_name(&row.id),
            id: row.id,
        })
        .collect();
    models.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    let current = align_current(parsed.current.unwrap_or_default(), &models);
    let error = if models.is_empty() {
        picker_error(parsed.error.as_deref())
    } else {
        String::new()
    };
    ModelsCatalog {
        models,
        current,
        error,
    }
}

/// Picker label for `id`. The kernel still receives the raw id.
///
/// A `:variant` tail (OpenRouter's `:batch`, `:free`) stays on the label as
/// a parenthesised tag, so `claude-fable-5.1` and `claude-fable-5.1:batch`
/// read as two rows instead of two "Fable 5.1".
pub fn model_display_name(id: &str) -> String {
    let id = id.trim();
    if id.is_empty() {
        return String::new();
    }
    let mut slug = id.rsplit('/').next().unwrap_or(id);
    let mut variant = "";
    if let Some((head, tail)) = slug.split_once(':') {
        slug = head;
        variant = tail.trim();
    }
    let mut parts: Vec<&str> = slug.split('-').filter(|part| !part.is_empty()).collect();
    if parts.len() >= 2 {
        let prefix = parts[0].to_ascii_lowercase();
        if matches!(
            prefix.as_str(),
            "claude" | "anthropic" | "openai" | "google"
        ) {
            parts.remove(0);
        }
    }
    let mut words: Vec<String> = Vec::new();
    for part in parts {
        if part.chars().all(|c| c.is_ascii_digit()) {
            if let Some(last) = words.last_mut() {
                if is_version(last) {
                    last.push('.');
                    last.push_str(part);
                    continue;
                }
            }
        }
        words.push(pretty_token(part));
    }
    let mut name = words.join(" ");
    if !variant.is_empty() {
        name.push_str(&format!(" ({variant})"));
    }
    name
}

fn is_version(s: &str) -> bool {
    let mut digit = false;
    for c in s.chars() {
        if c.is_ascii_digit() {
            digit = true;
        } else if c != '.' {
            return false;
        }
    }
    digit
}

fn pretty_token(part: &str) -> String {
    let lower = part.to_ascii_lowercase();
    match lower.as_str() {
        "gpt" | "glm" | "tts" | "api" => return lower.to_ascii_uppercase(),
        _ => {}
    }
    if let Some(rest) = lower.strip_suffix('b') {
        if !rest.is_empty() && is_version(rest) {
            return format!("{rest}B");
        }
    }
    if let Some(rest) = lower.strip_prefix('v') {
        if let Some(first) = rest.chars().next() {
            if first.is_ascii_digit() {
                return format!("V{rest}");
            }
        }
    }
    let mut chars = part.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut out = String::new();
    out.extend(first.to_uppercase());
    out.extend(chars.flat_map(|c| c.to_lowercase()));
    out
}

fn align_current(current: String, models: &[ModelOption]) -> String {
    if current.is_empty() || models.iter().any(|model| model.id == current) {
        return current;
    }
    let bare = current.rsplit('/').next().unwrap_or(&current);
    if models.iter().any(|model| model.id == bare) {
        return bare.to_string();
    }
    current
}

/// Catalog the rust kernel's provider will accept, read the way the kernel
/// reads it (`arbos_core::Host`: provider, base, key, model).
fn fetch_host_models() -> Option<ModelsCatalog> {
    let (base, key, current) = turn_host_auth()?;
    let url = format!("{}/models", base.trim_end_matches('/'));
    let client: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into();
    let mut req = client.get(&url);
    if !key.is_empty() {
        req = req.header("Authorization", &format!("Bearer {key}"));
    }
    for (name, value) in attribution_headers(ProviderKind::infer(&base)) {
        req = req.header(*name, *value);
    }
    let Ok(mut resp) = req.call() else {
        return None;
    };
    let Ok(body) = resp.body_mut().read_to_string() else {
        return None;
    };
    let Ok(parsed) = serde_json::from_str::<UpstreamModels>(&body) else {
        return None;
    };
    let mut models: Vec<ModelOption> = parsed
        .data
        .into_iter()
        .filter(UpstreamModel::usable)
        .map(|row| ModelOption {
            name: model_display_name(&row.id),
            id: row.id,
        })
        .collect();
    models.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    if models.is_empty() {
        return None;
    }
    let current = align_current(current, &models);
    Some(ModelsCatalog {
        models,
        current,
        error: String::new(),
    })
}

/// `(base, key, model)` for the turn host, or None when there is no key —
/// then there is no catalog to fetch and the gateway list is the fallback.
fn turn_host_auth() -> Option<(String, String, String)> {
    let host = Host::peek().ok()?;
    let key = host.api_key()?;
    let base = host.config.api_base().ok()?;
    Some((base, key, host.config.model()))
}

/// What the Model settings section shows: the provider, where the key is,
/// and the model turns use when a chat says `inherit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSummary {
    pub provider: ProviderKind,
    pub base: String,
    pub model: String,
    pub key: KeySource,
    pub config_path: PathBuf,
    /// A malformed config.toml, verbatim, so the user can fix it.
    pub error: Option<String>,
}

pub fn host_summary() -> HostSummary {
    match Host::peek() {
        Ok(host) => HostSummary {
            provider: host.config.provider(),
            base: host
                .config
                .api_base()
                .unwrap_or_else(|e| format!("({e:#})")),
            model: host.config.model(),
            key: host.key_source(),
            config_path: host.config_path(),
            error: None,
        },
        Err(e) => {
            let dir = arbos_core::host::dirs_config();
            let cfg = HostConfig::default();
            HostSummary {
                provider: cfg.provider(),
                base: cfg.api_base().unwrap_or_default(),
                model: cfg.model(),
                key: KeySource::Missing(cfg.key_env()),
                config_path: dir.join("config.toml"),
                error: Some(format!("{e:#}")),
            }
        }
    }
}

/// Save the provider choice into config.toml. A change resets the base,
/// key variable, and model to that provider's defaults, as setup does.
pub fn save_host_provider(provider: ProviderKind) -> Result<HostSummary> {
    let mut host = Host::peek()?;
    if host.config.provider() != provider {
        host.config.set_provider(provider);
    }
    host.config.provider = Some(provider);
    host.save()?;
    Ok(host_summary())
}

/// Is `key` accepted by the configured provider? Blocking; run it off the
/// main thread. The same check `arbos-kernel setup` makes: OpenRouter's
/// `/key` (its `/models` is public), `/models` elsewhere.
pub fn check_host_key(key: &str) -> Result<()> {
    let host = Host::peek()?;
    let base = host.config.api_base()?;
    let path = match ProviderKind::infer(&base) {
        ProviderKind::OpenRouter => "/key",
        ProviderKind::OpenAi | ProviderKind::Custom => "/models",
    };
    let client: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut req = client
        .get(&format!("{base}{path}"))
        .header("Authorization", &format!("Bearer {}", key.trim()));
    for (name, value) in attribution_headers(ProviderKind::infer(&base)) {
        req = req.header(*name, *value);
    }
    let resp = req.call().with_context(|| format!("reach {base}"))?;
    let status = resp.status().as_u16();
    if (200..300).contains(&status) {
        return Ok(());
    }
    Err(anyhow!(match status {
        401 => "the key was rejected".to_string(),
        402 => "the account has no credit".to_string(),
        403 => "the key is not allowed here".to_string(),
        other => format!("{base} answered {other}"),
    }))
}

/// Save a pasted key into config.toml the way `arbos-kernel setup` does:
/// owner-readable file, key never echoed. An empty key clears the saved
/// one so the environment variable is read again.
pub fn save_host_key(key: &str) -> Result<HostSummary> {
    let mut host = Host::peek()?;
    let key = key.trim();
    host.config.api_key = (!key.is_empty()).then(|| key.to_string());
    host.save()?;
    Ok(host_summary())
}

/// Set the base URL requests go to. Empty = the provider's default (a
/// custom provider needs one). The trailing slash is dropped.
pub fn save_host_base(base: &str) -> Result<HostSummary> {
    let mut host = Host::peek()?;
    let base = base.trim().trim_end_matches('/');
    if !base.is_empty() && !(base.starts_with("http://") || base.starts_with("https://")) {
        anyhow::bail!("the base URL must start with http:// or https://");
    }
    host.config.api_base = base.to_string();
    host.save()?;
    Ok(host_summary())
}

/// Set the model turns use by default. Empty = the provider's default.
pub fn save_host_model(model: &str) -> Result<HostSummary> {
    let mut host = Host::peek()?;
    host.config.model = model.trim().to_string();
    host.save()?;
    Ok(host_summary())
}

fn picker_error(raw: Option<&str>) -> String {
    let err = raw.unwrap_or("").trim();
    if err.is_empty() {
        return String::new();
    }
    let short = err.strip_prefix("models catalog: ").unwrap_or(err).trim();
    if short.is_empty() {
        "No models".into()
    } else {
        short.to_string()
    }
}

/// One chat the kernel still holds — title, name, and its durable id.
#[derive(Debug, Clone, Default)]
pub struct SessionSummary {
    pub id: String,
    pub name: String,
    pub title: String,
    pub updated_ms: i64,
    /// Kernel id of the parent agent. Empty for a root.
    pub parent: Option<String>,
}

/// What this place still holds, and whether the listing reached a live
/// source. `reached` is false when ssh or the folder could not be read —
/// the UI must not treat an empty list as "delete everything".
#[derive(Debug, Clone, Default)]
pub struct PlaceSessions {
    pub rows: Vec<SessionSummary>,
    pub reached: bool,
}

/// Chats the live kernel has for `place`. `spawn` starts a kernel when none
/// answers, so a launch can reattach; a poll must not.
///
/// Agent folders (`<workspace>/.arbos/agents/`) belong to this rust kernel
/// place. HTTP `GET /api/sessions` is the Go / Mac gateway: include it only
/// when that process's cwd is this same workspace. A leftover `web.json`
/// (or a tunnel to another kernel) must not dump another Arbos's chats
/// into this sidebar.
pub fn list_sessions(place: &Place, spawn: bool) -> PlaceSessions {
    if spawn {
        let _ = attach_or_spawn_place(place);
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut reached = false;
    match list_place_agents(place) {
        Some(rows) => {
            reached = true;
            for row in rows {
                if seen.insert(row.id.clone()) {
                    out.push(row);
                }
            }
        }
        None => {}
    }
    if gateway_serves_place(place)
        && let Some(base) = http_base_place(place)
    {
        reached = true;
        for row in list_http_sessions(&base) {
            if seen.insert(row.id.clone()) {
                out.push(row);
            }
        }
    }
    PlaceSessions { rows: out, reached }
}

/// Go / Mac session ids (`sess-…`, Telegram `tg-…`). They live in
/// `sessions.db`, not in this rust kernel's agent folders.
pub fn go_kernel_id(id: &str) -> bool {
    id.starts_with("sess-") || id.starts_with("tg-")
}

fn list_http_sessions(base: &str) -> Vec<SessionSummary> {
    let Ok(mut resp) = http().get(&format!("{base}/api/sessions")).call() else {
        return Vec::new();
    };
    let Ok(body) = resp.body_mut().read_to_string() else {
        return Vec::new();
    };
    let Ok(parsed) = serde_json::from_str::<SessionsBody>(&body) else {
        return Vec::new();
    };
    parsed
        .sessions
        .into_iter()
        .filter(|row| !row.id.is_empty())
        .map(|row| {
            let title = row.title.unwrap_or_default();
            let name = row.name.unwrap_or_default();
            SessionSummary {
                id: row.id,
                name: name.clone(),
                title: if title.is_empty() { name } else { title },
                updated_ms: row.updated_at.unwrap_or(0),
                parent: None,
            }
        })
        .collect()
}

fn list_place_agents(place: &Place) -> Option<Vec<SessionSummary>> {
    match &place.host {
        None => list_local_agents(&place.path),
        Some(_) => list_remote_agents(place),
    }
}

fn list_remote_agents(place: &Place) -> Option<Vec<SessionSummary>> {
    let host = place.host.as_deref()?;
    let dir = shell_path(&place.path.to_string_lossy());
    let script = format!(
        r#"d={dir}/.arbos/agents
[ -d "$d" ] || exit 0
for p in "$d"/*; do
  [ -d "$p" ] || continue
  id=$(basename "$p")
  name=$id
  parent=
  if [ -f "$p/agent.md" ]; then
    n=$(awk -F': *' '/^name:/ {{print $2; exit}}' "$p/agent.md")
    [ -n "$n" ] && name=$n
    parent=$(awk -F': *' '/^parent:/ {{print $2; exit}}' "$p/agent.md")
  fi
  printf '%s\t%s\t%s\n' "$id" "$name" "$parent"
done"#,
        dir = dir,
    );
    let out = ssh_run(host, &script).ok()?;
    if out.status != 0 {
        return None;
    }
    Some(
        out.stdout
            .lines()
            .filter_map(|line| {
                let mut parts = line.splitn(3, '\t');
                let id = parts.next()?.trim();
                let name = parts.next().unwrap_or("").trim();
                let parent = parts.next().unwrap_or("").trim();
                if id.is_empty() || !safe_session_id(id) {
                    return None;
                }
                Some(SessionSummary {
                    id: id.to_string(),
                    // The script echoes the id when agent.md has no name.
                    name: (name != id).then(|| name.to_string()).unwrap_or_default(),
                    title: String::new(),
                    updated_ms: 0,
                    parent: (!parent.is_empty()).then(|| parent.to_string()),
                })
            })
            .collect(),
    )
}

fn list_local_agents(path: &Path) -> Option<Vec<SessionSummary>> {
    let dir = path.join(".arbos").join("agents");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Some(Vec::new());
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        let md = std::fs::read_to_string(entry.path().join("agent.md")).unwrap_or_default();
        let front = agent_front(&md);
        let parent = front.parent;
        let name = front.name.unwrap_or_default();
        let title = front
            .title
            .filter(|t| !arbos_core::chattitle::is_generic(t, Some(&id)))
            .or_else(|| first_transcript_title(&entry.path().join("transcript.jsonl")))
            .unwrap_or_default();
        let updated_ms = entry
            .path()
            .join("transcript.jsonl")
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        out.push(SessionSummary {
            id: id.clone(),
            name,
            title,
            updated_ms,
            parent,
        });
    }
    Some(out)
}

/// What the kernel called one agent, read off its `agent.md`. Local
/// places only; a remote child is named when the listing next runs.
/// `mode:` from a local agent's `agent.md`, for the composer's Mode switch.
pub fn agent_mode(place: &Place, id: &str) -> Option<String> {
    if place.host.is_some() || !safe_session_id(id) {
        return None;
    }
    let dir = place.path.join(".arbos").join("agents").join(id);
    let agent = arbos_core::Agent::load(&dir).ok()?;
    Some(agent.mode.as_str().to_string())
}

pub fn agent_name(place: &Place, id: &str) -> Option<String> {
    if place.host.is_some() || !safe_session_id(id) {
        return None;
    }
    let md = std::fs::read_to_string(
        place
            .path
            .join(".arbos")
            .join("agents")
            .join(id)
            .join("agent.md"),
    )
    .ok()?;
    agent_front(&md)
        .name
        .filter(|name| !arbos_core::chattitle::is_generic(name, Some(id)))
}

/// The fields of `agent.md` the sidebar reads. `name` is what the kernel
/// called the agent — for a spawned child, the brief it was given.
#[derive(Default)]
struct AgentFront {
    name: Option<String>,
    title: Option<String>,
    parent: Option<String>,
}

fn agent_front(text: &str) -> AgentFront {
    let mut front = AgentFront::default();
    let field = |value: &str| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    };
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("name:") {
            front.name = field(value);
        } else if let Some(value) = line.strip_prefix("title:") {
            front.title = field(value);
        } else if let Some(value) = line.strip_prefix("parent:") {
            front.parent = field(value);
        }
    }
    front
}

fn first_transcript_title(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let Ok(ev) = serde_json::from_str::<arbos_core::Event>(line) else {
            continue;
        };
        if let Some(prompt) = ev.user_text()
            && let Some(title) = arbos_core::chattitle::from_prompt(prompt)
        {
            return Some(title);
        }
    }
    None
}

/// New desktop chat: its own agent folder, not `root`.
pub fn mint_chat(place: &Place) -> Result<String> {
    if place.host.is_some() {
        return Ok(arbos_core::ROOT_ID.to_string());
    }
    let core_place = arbos_core::Place::new(&place.path);
    arbos_core::create_chat(&core_place).map(|agent| agent.id.to_string())
}

/// Write a generated title onto the agent so a later listing sees it.
pub fn set_chat_title(place: &Place, id: &str, title: &str) {
    if place.host.is_some() || !safe_session_id(id) || title.is_empty() {
        return;
    }
    let dir = place.path.join(".arbos").join("agents").join(id);
    let Ok(mut agent) = arbos_core::Agent::load(&dir) else {
        return;
    };
    if agent.title == title {
        return;
    }
    agent.title = title.to_string();
    let _ = agent.save(&dir);
}

/// If the window has a transcript the kernel folder never stored, write
/// those user/assistant lines so turn 2 sees turn 1.
pub fn seed_transcript(place: &Place, id: &str, items: &[crate::model::session::ChatItem]) {
    if place.host.is_some() || !safe_session_id(id) || items.is_empty() {
        return;
    }
    let path = place
        .path
        .join(".arbos")
        .join("agents")
        .join(id)
        .join("transcript.jsonl");
    // Only a folder that stored nothing gets seeded. Checking for a User
    // event alone missed child agents — their wakes are `say` and `plan`,
    // never `user` — and re-seeded their whole history on every attach.
    let existing = arbos_core::load_transcript(&path).unwrap_or_default();
    if !existing.is_empty() {
        return;
    }
    let mut batch = Vec::new();
    for item in items {
        match item {
            crate::model::session::ChatItem::User(message) => {
                let attachments: Vec<String> =
                    message.files.iter().map(|f| f.path.clone()).collect();
                batch.push(arbos_core::Event::new(arbos_core::EventKind::User {
                    text: message.text.clone(),
                    attachments,
                    channel: String::new(),
                    device: String::new(),
                }));
            }
            crate::model::session::ChatItem::From { who, text, .. } => {
                batch.push(arbos_core::Event::new(arbos_core::EventKind::Say {
                    from: who.clone(),
                    text: text.clone(),
                }));
            }
            crate::model::session::ChatItem::Agent(text) => {
                batch.push(arbos_core::Event::new(arbos_core::EventKind::Assistant {
                    text: text.clone(),
                    reasoning_details: None,
                }));
            }
            crate::model::session::ChatItem::Notice { text, failed } => {
                batch.push(arbos_core::Event::new(arbos_core::EventKind::Notice {
                    text: text.clone(),
                    failed: *failed,
                }));
            }
            _ => {}
        }
    }
    let _ = arbos_core::append_events(&path, &batch);
}

/// True when this place's `web.json` names a live Go kernel whose cwd is
/// this workspace. A pid that is alive but sitting in another folder is
/// another Arbos — do not take its session list.
fn gateway_serves_place(place: &Place) -> bool {
    let key = place.encode();
    {
        let map = gateway_cache();
        if let Some(hit) = map.get(&key)
            && hit.at.elapsed() < Duration::from_secs(5)
        {
            return hit.ok;
        }
    }
    let ok = match &place.host {
        None => local_gateway_serves(&place.path),
        Some(host) => remote_gateway_serves(host, &place.path),
    };
    gateway_cache().insert(
        key,
        GateHit {
            at: Instant::now(),
            ok,
        },
    );
    ok
}

struct GateHit {
    at: Instant,
    ok: bool,
}

fn gateway_cache() -> std::sync::MutexGuard<'static, HashMap<String, GateHit>> {
    static CACHE: OnceLock<Mutex<HashMap<String, GateHit>>> = OnceLock::new();
    CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn local_gateway_serves(workspace: &Path) -> bool {
    let Some(info) = read_json_info(&gateway_json(workspace)).filter(alive) else {
        return false;
    };
    let Some(cwd) = process_cwd(info.pid) else {
        return false;
    };
    paths_same_workspace(workspace, &cwd)
}

fn remote_gateway_serves(host: &str, path: &Path) -> bool {
    let dir = shell_path(&path.to_string_lossy());
    let script = format!(
        r#"f={dir}/.arbos/web.json
[ -f "$f" ] || exit 1
pid=$(sed -n 's/.*"pid":\([0-9]*\).*/\1/p' "$f")
[ -n "$pid" ] && kill -0 "$pid" 2>/dev/null || exit 1
cwd=$(readlink /proc/"$pid"/cwd 2>/dev/null || true)
if [ -z "$cwd" ]; then
  cwd=$(lsof -a -p "$pid" -d cwd -Fn 2>/dev/null | sed -n 's/^n//p' | tail -1)
fi
[ -n "$cwd" ] || exit 1
real=$(realpath -m -- {dir}) || exit 1
[ "$cwd" = "$real" ]"#,
        dir = dir,
    );
    ssh_run(host, &script).is_ok_and(|out| out.status == 0)
}

fn process_cwd(pid: i32) -> Option<String> {
    if pid <= 0 {
        return None;
    }
    let out = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "cwd="])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let cwd = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!cwd.is_empty()).then_some(cwd)
}

fn paths_same_workspace(place: &Path, cwd: &str) -> bool {
    let left = normalize_workspace(place);
    let right = normalize_workspace(Path::new(cwd));
    !left.is_empty() && left == right
}

fn normalize_workspace(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let expanded = if raw == "~" {
        dirs::home_dir()
            .map(|home| home.to_string_lossy().into_owned())
            .unwrap_or_else(|| raw.into_owned())
    } else if let Some(rest) = raw.strip_prefix("~/") {
        dirs::home_dir()
            .map(|home| home.join(rest).to_string_lossy().into_owned())
            .unwrap_or_else(|| raw.into_owned())
    } else {
        raw.into_owned()
    };
    let path = PathBuf::from(expanded);
    let canon = path.canonicalize().unwrap_or(path);
    canon.to_string_lossy().trim_end_matches('/').to_string()
}

/// Transcript the kernel has for one chat. `None` when it cannot be read —
/// the local copy must then stay.
pub fn session_history(place: &Place, id: &str) -> Option<crate::model::history::Replay> {
    // Go `arbos web` answers `/api/sessions/{id}/events` with
    // `{"events":[],"session":null}` for a rust-kernel id it does not
    // own. That is not a transcript — fall through to the agent folder.
    // Only ask a gateway whose cwd is this place, or another Arbos's
    // empty events would hide the rust transcript.
    if go_kernel_id(id)
        && gateway_serves_place(place)
        && let Some(base) = http_base_place(place)
        && list_http_sessions(&base).iter().any(|row| row.id == id)
        && let Ok(replay) = crate::model::history::load(&base, id)
    {
        return Some(replay);
    }
    if place.host.is_some() {
        return None;
    }
    let path = place
        .path
        .join(".arbos")
        .join("agents")
        .join(id)
        .join("transcript.jsonl");
    let text = std::fs::read_to_string(path).ok()?;
    let mut items = Vec::new();
    // Timestamps give the replay what the live view measures: the turn's
    // wall time on its prompt, and a thought's seconds as the gap to the
    // event after it.
    let mut turn_began: Option<i64> = None;
    let mut thinking_since: Option<i64> = None;
    for line in text.lines() {
        if let Ok(ev) = serde_json::from_str::<arbos_core::Event>(line) {
            let thinking = matches!(ev.kind, arbos_core::EventKind::Thinking { .. });
            if !thinking {
                if let Some(since) = thinking_since.take() {
                    if let Some(crate::model::session::ChatItem::Thinking { secs, .. }) =
                        items.last_mut()
                    {
                        *secs = secs_between(since, ev.ts);
                    }
                }
            }
            match &ev.kind {
                arbos_core::EventKind::User { .. } => turn_began = Some(ev.ts),
                arbos_core::EventKind::Thinking { .. } => {
                    thinking_since.get_or_insert(ev.ts);
                }
                arbos_core::EventKind::TurnComplete { .. } => {
                    if let Some(began) = turn_began.take() {
                        if let Some(crate::model::session::ChatItem::User(message)) = items
                            .iter_mut()
                            .rev()
                            .find(|item| matches!(item, crate::model::session::ChatItem::User(_)))
                        {
                            message.worked_secs = secs_between(began, ev.ts);
                        }
                    }
                }
                _ => {}
            }
            if let Some(item) = event_to_item(&ev) {
                match (&item, items.last_mut()) {
                    (
                        crate::model::session::ChatItem::Thinking { text, .. },
                        Some(crate::model::session::ChatItem::Thinking {
                            text: held, done, ..
                        }),
                    ) => {
                        held.push_str(text);
                        *done = true;
                    }
                    _ => items.push(item),
                }
            }
        }
    }
    Some(crate::model::history::Replay {
        items,
        model: agent_model(&arbos_core::Place::new(&place.path), id),
    })
}

/// Whole seconds from `from` to `to` (unix millis); `None` when either is
/// missing (older transcripts wrote no timestamps) or the order is wrong.
fn secs_between(from: i64, to: i64) -> Option<u32> {
    if from <= 0 || to <= 0 || to < from {
        return None;
    }
    Some(((to - from) / 1000).min(u32::MAX as i64) as u32)
}

/// The model the kernel keeps for this agent in `agent.md`, when it is
/// not `inherit`. The chip shows it, and a reopen does not fall back to
/// the config default while the kernel keeps using the chosen one.
pub fn agent_model(place: &arbos_core::Place, id: &str) -> Option<String> {
    let agent = arbos_core::Agent::load(&place.agent_dir(id)).ok()?;
    let model = agent.model.trim();
    (!model.is_empty() && model != "inherit").then(|| model.to_string())
}

fn event_to_item(ev: &arbos_core::Event) -> Option<crate::model::session::ChatItem> {
    use crate::model::session::ChatItem;
    match &ev.kind {
        arbos_core::EventKind::User {
            text, attachments, ..
        } => {
            let mut message = crate::model::attachment::UserMessage::from(text.clone());
            for path in attachments {
                message.add_file_path(path);
            }
            message.sent_at = (ev.ts > 0).then_some(ev.ts);
            Some(ChatItem::User(message))
        }
        // An empty line is a step boundary for the kernel's projection, not
        // something the model said.
        arbos_core::EventKind::Assistant { text, .. } if text.trim().is_empty() => None,
        arbos_core::EventKind::Assistant { text, .. } => Some(ChatItem::Agent(text.clone())),
        arbos_core::EventKind::Say { from, text } => Some(ChatItem::From {
            who: from.clone(),
            text: text.clone(),
            images: Vec::new(),
        }),
        arbos_core::EventKind::Notice { text, failed } => Some(ChatItem::Notice {
            text: text.clone(),
            failed: *failed,
        }),
        // The turn was cut short: by the Stop button, a stop word, a Force,
        // or the kernel. A line in the pane, and the turn's fold says so.
        arbos_core::EventKind::Interrupted { detail } => Some(ChatItem::Notice {
            text: crate::model::session::interrupt_label(detail),
            failed: false,
        }),
        arbos_core::EventKind::Thinking { text } if text.trim().is_empty() => None,
        arbos_core::EventKind::Thinking { text } => Some(ChatItem::Thinking {
            text: text.clone(),
            done: true,
            secs: None,
        }),
        arbos_core::EventKind::Tool(rec) => {
            let hint = crate::agent::acp::tool_hint(&rec.name, &rec.paths, rec.args.as_ref());
            Some(ChatItem::Tool {
                id: rec.call_id.clone(),
                kind: crate::agent::acp::tool_kind(&rec.name),
                label: crate::agent::acp::tool_title(&rec.name, hint.as_deref()),
                status: if rec.error.is_some() {
                    session::ToolStatus::Failure
                } else {
                    session::ToolStatus::Success
                },
                output: rec
                    .error
                    .clone()
                    .or_else(|| rec.body.clone())
                    .unwrap_or_default(),
                diff: crate::agent::acp::display_diff(
                    &rec.name,
                    rec.args.as_ref(),
                    rec.diff.as_deref(),
                ),
                child_session: rec.child.clone(),
                secs: rec
                    .started
                    .zip(rec.ended)
                    .map(|(started, ended)| ((ended - started).max(0) / 1000) as u32),
            })
        }
        _ => None,
    }
}

/// Tell the kernel the name the user typed, so Mac and `say to=` agree.
pub fn rename_session(place: &Place, id: &str, name: &str) {
    let Some(base) = http_base_place(place) else {
        return;
    };
    let body = serde_json::json!({ "name": name }).to_string();
    let _ = http()
        .patch(&format!("{base}/api/sessions/{id}"))
        .content_type("application/json")
        .send(body);
}

/// Drop a chat from the kernel store. Best effort, never waits on attach:
/// the UI has already dismissed the row. HTTP is this place's gateway
/// only, 2s timeout. Go ids skip the agent-folder rm (that path is ssh
/// on a remote and those folders are not theirs).
pub fn delete_session(place: &Place, id: &str) -> Result<()> {
    if !safe_session_id(id) {
        return Err(anyhow!("bad session id"));
    }
    if gateway_serves_place(place)
        && let Some(base) = http_base_place(place)
    {
        let _ = http().delete(&format!("{base}/api/sessions/{id}")).call();
    }
    if !go_kernel_id(id) {
        let _ = delete_agent_dir(place, id);
    }
    Ok(())
}

fn delete_agent_dir(place: &Place, id: &str) -> Result<()> {
    if !safe_session_id(id) {
        return Err(anyhow!("bad session id"));
    }
    match &place.host {
        None => {
            let dir = place.path.join(".arbos").join("agents").join(id);
            if dir.exists() {
                std::fs::remove_dir_all(&dir)
                    .with_context(|| format!("remove {}", dir.display()))?;
            }
            // New chats attach as `root`. Wiping that folder and leaving
            // it gone is a silent no-reply on the next send.
            if id == arbos_core::ROOT_ID {
                let _ = arbos_core::bootstrap(&arbos_core::Place::new(&place.path));
            }
            Ok(())
        }
        Some(host) => {
            let dir = shell_path(&place.path.to_string_lossy());
            let script = format!("rm -rf {dir}/.arbos/agents/{id}");
            let out = ssh_run_brief(host, &script)?;
            if out.status != 0 {
                return Err(anyhow!("{}", out.problem()));
            }
            Ok(())
        }
    }
}

fn safe_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Copy a kernel chat into a new root session. The source socket stays
/// bound. Returns the new session id.
pub fn clone_session(place: &Place, source_id: &str) -> Result<String> {
    // A local place is served by the Rust kernel, whose agents are folders:
    // a fork is a new chat folder carrying the source's transcript. The
    // kernel lists agents from disk, so it sees the copy on its next scan.
    // Only a remote place still goes through the Go gateway's `clone`.
    if place.host.is_none() && safe_session_id(source_id) {
        return fork_chat_folder(place, source_id);
    }
    let info = attach_or_spawn_place(place)?;
    let url = websocket_url(&info);
    let source = source_id.to_owned();
    crate::agent::acp::runtime().block_on(clone_over_ws(&url, &source))
}

fn fork_chat_folder(place: &Place, source_id: &str) -> Result<String> {
    let core_place = arbos_core::Place::new(&place.path);
    let source_dir = core_place.agent_dir(source_id);
    let source = arbos_core::Agent::load(&source_dir)
        .with_context(|| format!("fork: no chat {source_id} in {}", place.path.display()))?;
    let mut agent = arbos_core::create_chat(&core_place)?;
    agent.model = source.model.clone();
    agent.allowlist = source.allowlist.clone();
    agent.title = if source.title.is_empty() {
        String::new()
    } else {
        format!("{} (fork)", source.title)
    };
    let id = agent.id.to_string();
    let dir = core_place.agent_dir(&id);
    agent.save(&dir)?;
    let from = arbos_core::files::Layout::new(&core_place, source_id).transcript();
    if from.exists() {
        let to = arbos_core::files::Layout::new(&core_place, &id).transcript();
        std::fs::copy(&from, &to).with_context(|| format!("fork: copy {}", from.display()))?;
    }
    Ok(id)
}

async fn clone_over_ws(url: &str, source_id: &str) -> Result<String> {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::{
        connect_async,
        tungstenite::{Message, Utf8Bytes},
    };
    let (ws, _) = tokio::time::timeout(Duration::from_secs(10), connect_async(url))
        .await
        .map_err(|_| anyhow!("clone timed out"))?
        .map_err(|e| anyhow!("clone websocket: {e}"))?;
    let (mut sink, mut stream) = ws.split();
    let frame = serde_json::json!({ "type": "clone", "session_id": source_id }).to_string();
    sink.send(Message::Text(Utf8Bytes::from(frame)))
        .await
        .map_err(|e| anyhow!("clone send: {e}"))?;
    let deadline = tokio::time::sleep(Duration::from_secs(10));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => return Err(anyhow!("clone timed out")),
            frame = stream.next() => {
                let Some(Ok(Message::Text(text))) = frame else {
                    return Err(anyhow!("clone closed"));
                };
                let value: serde_json::Value = serde_json::from_str(&text)?;
                match value.get("type").and_then(serde_json::Value::as_str) {
                    Some("cloned") => {
                        let id = value
                            .get("session_id")
                            .and_then(serde_json::Value::as_str)
                            .filter(|id| !id.is_empty())
                            .ok_or_else(|| anyhow!("clone missing session_id"))?;
                        return Ok(id.to_owned());
                    }
                    Some("error") => {
                        let err = value
                            .get("error")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("clone failed");
                        return Err(anyhow!("{err}"));
                    }
                    _ => {}
                }
            }
        }
    }
}

#[derive(Deserialize)]
struct SessionsBody {
    #[serde(default)]
    sessions: Vec<SessionRow>,
}

#[derive(Deserialize)]
struct SessionRow {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    updated_at: Option<i64>,
}

#[derive(Deserialize)]
struct ModelsBody {
    #[serde(default)]
    models: Option<Vec<ModelRow>>,
    #[serde(default)]
    current: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct UpstreamModels {
    #[serde(default)]
    data: Vec<UpstreamModel>,
}

#[derive(Deserialize)]
struct UpstreamModel {
    #[serde(default)]
    id: String,
    /// OpenRouter lists what each model accepts. Absent on other hosts.
    #[serde(default)]
    supported_parameters: Vec<String>,
}

impl UpstreamModel {
    /// Whether a turn can use this model. The kernel sends its tool
    /// schemas on every call, so a model with no tool-capable endpoint
    /// 404s on the first step; `:batch` variants only answer through the
    /// batch API, never in a live chat.
    fn usable(&self) -> bool {
        if self.id.is_empty() || self.id.ends_with(":batch") {
            return false;
        }
        self.supported_parameters.is_empty()
            || self.supported_parameters.iter().any(|p| p == "tools")
    }
}

#[derive(Deserialize)]
struct ModelRow {
    #[serde(default)]
    id: String,
}

#[derive(Deserialize)]
struct CommandsBody {
    #[serde(default)]
    commands: Option<Vec<CommandRow>>,
}

#[derive(Deserialize)]
struct CommandRow {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: Option<String>,
}

fn load_skills_dir(dir: &Path, include_root_md: bool) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let skill_md = dir.join("SKILL.md");
    if skill_md.is_file() {
        return load_skill_file(&skill_md).into_iter().collect();
    }
    let mut out = Vec::new();
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            out.extend(load_skills_dir(&path, false));
            continue;
        }
        if include_root_md && name.ends_with(".md") {
            if let Some(row) = load_skill_file(&path) {
                out.push(row);
            }
        }
    }
    out
}

fn load_skill_file(path: &Path) -> Option<(String, String)> {
    let raw = std::fs::read_to_string(path).ok()?;
    let fm = frontmatter(&raw);
    let description = fm.get("description")?.trim().to_string();
    if description.is_empty() {
        return None;
    }
    let name = fm
        .get("name")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            let base = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if base.eq_ignore_ascii_case("SKILL.md") {
                path.parent()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string()
            } else {
                base.trim_end_matches(".md").to_string()
            }
        });
    Some((name, description))
}

fn load_prompt_dir(dir: &Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.extension().is_some_and(|ext| ext == "md") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let fm = frontmatter(&raw);
        let name = path
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let description = fm
            .get("description")
            .cloned()
            .filter(|s| !s.is_empty())
            .unwrap_or_default();
        if !name.is_empty() {
            out.push((name, description));
        }
    }
    out
}

fn frontmatter(raw: &str) -> HashMap<String, String> {
    let raw = raw.replace("\r\n", "\n").replace('\r', "\n");
    let Some(rest) = raw.strip_prefix("---") else {
        return HashMap::new();
    };
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let Some(end) = rest.find("\n---") else {
        return HashMap::new();
    };
    let mut out = HashMap::new();
    for line in rest[..end].lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        out.insert(key.trim().to_string(), value);
    }
    out
}

/// What is running for every chat right now. Empty when the kernel is
/// quiet or unreachable.
pub fn live_by_session(place: &Place) -> HashMap<String, Vec<LiveWork>> {
    if !gateway_serves_place(place) {
        return HashMap::new();
    }
    let Some(base) = http_base_place(place) else {
        return HashMap::new();
    };
    let Ok(mut resp) = http().get(&format!("{base}/api/activity")).call() else {
        return HashMap::new();
    };
    let Ok(body) = resp.body_mut().read_to_string() else {
        return HashMap::new();
    };
    let Ok(act) = serde_json::from_str::<Activity>(&body) else {
        return HashMap::new();
    };
    let mut goals: HashMap<(String, i64), String> = HashMap::new();
    for row in &act.standing {
        let Some(chat) = row.chat.as_deref().filter(|chat| !chat.is_empty()) else {
            continue;
        };
        if row.goal.is_empty() {
            continue;
        }
        goals.insert((chat.to_string(), row.node), row.goal.clone());
    }
    let mut out: HashMap<String, Vec<LiveWork>> = HashMap::new();
    for run in act.runs {
        if !run.active || run.chat.is_empty() {
            continue;
        }
        let label = run
            .node
            .and_then(|node| goals.get(&(run.chat.clone(), node)).cloned())
            .filter(|goal| !goal.is_empty())
            .unwrap_or_else(|| match run.kind.as_str() {
                "scheduled" => "scheduled".into(),
                _ => "sub-agent".into(),
            });
        out.entry(run.chat).or_default().push(LiveWork {
            label,
            running: true,
        });
    }
    out
}

/// What is running for `session` right now. Empty when the kernel is quiet
/// or unreachable — the line above the composer is then not drawn.
pub fn live_work(place: &Place, session: &str) -> Vec<LiveWork> {
    live_by_session(place).remove(session).unwrap_or_default()
}

/// Kernel session ids that belong under `session`: scheduled children and
/// activity runs the parent owns.
pub fn child_sessions(place: &Place, session: &str) -> Vec<String> {
    child_sessions_many(place, &[session.to_string()])
        .remove(session)
        .unwrap_or_default()
}

/// Children for every parent in one activity fetch, plus one `/children`
/// call each. The 2s poll uses this so background chats see new delegates
/// without a transcript refetch.
pub fn child_sessions_many(place: &Place, sessions: &[String]) -> HashMap<String, Vec<String>> {
    let mut out = HashMap::new();
    if sessions.is_empty() {
        return out;
    }
    if let Some(rows) = list_place_agents(place) {
        for parent in sessions {
            let kids: Vec<String> = rows
                .iter()
                .filter(|row| row.parent.as_deref() == Some(parent.as_str()))
                .map(|row| row.id.clone())
                .collect();
            if !kids.is_empty() {
                out.insert(parent.clone(), kids);
            }
        }
    }
    if !gateway_serves_place(place) {
        return out;
    }
    let Some(base) = http_base_place(place) else {
        return out;
    };
    let activity = http()
        .get(&format!("{base}/api/activity"))
        .call()
        .ok()
        .and_then(|mut resp| resp.body_mut().read_to_string().ok())
        .and_then(|body| serde_json::from_str::<Activity>(&body).ok());
    for session in sessions {
        let mut kids = out.remove(session).unwrap_or_default();
        let mut seen: HashMap<String, ()> = kids.iter().cloned().map(|id| (id, ())).collect();
        let mut add = |id: String| {
            if id.is_empty() || seen.contains_key(&id) {
                return;
            }
            seen.insert(id.clone(), ());
            kids.push(id);
        };
        if let Ok(mut resp) = http()
            .get(&format!("{base}/api/sessions/{session}/children"))
            .call()
        {
            if let Ok(body) = resp.body_mut().read_to_string() {
                if let Ok(parsed) = serde_json::from_str::<ChildrenBody>(&body) {
                    for row in parsed.children {
                        add(row.id);
                    }
                }
            }
        }
        if let Some(act) = &activity {
            for run in &act.runs {
                if run.chat == *session {
                    add(run.id.clone());
                }
            }
        }
        out.insert(session.clone(), kids);
    }
    out
}

#[derive(Deserialize)]
struct ChildrenBody {
    #[serde(default)]
    children: Vec<ChildRow>,
}

#[derive(Deserialize)]
struct ChildRow {
    #[serde(default)]
    id: String,
}

#[derive(Deserialize)]
struct Activity {
    #[serde(default)]
    standing: Vec<StandingRow>,
    #[serde(default)]
    runs: Vec<RunRow>,
}

#[derive(Deserialize)]
struct StandingRow {
    #[serde(default)]
    node: i64,
    #[serde(default)]
    goal: String,
    #[serde(default)]
    chat: Option<String>,
}

#[derive(Deserialize)]
struct RunRow {
    #[serde(default)]
    id: String,
    #[serde(default)]
    chat: String,
    #[serde(default)]
    node: Option<i64>,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    active: bool,
}

/// Press the composer's mic. Capture runs on this Mac, not the kernel.
/// `voice_url` / `voice_token` (or `voice_token_env`) from config.toml:
/// the self-hosted speech server. `None` when no URL is set, in which case
/// dictation falls back to this Mac's helper.
pub fn voice_config() -> Option<crate::voice_ws::VoiceCfg> {
    let text = std::fs::read_to_string(arbos_core::host_dir().join("config.toml")).ok()?;
    let mut url = String::new();
    let mut token = String::new();
    let mut token_env = String::new();
    let mut mirror = true;
    let mut reply = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim().trim_matches('"').trim_matches('\'');
        match k.trim() {
            "voice_url" => url = v.to_string(),
            "voice_token" => token = v.to_string(),
            "voice_token_env" => token_env = v.to_string(),
            "voice_mirror" => mirror = !matches!(v, "false" | "0" | "no"),
            "voice_reply" => reply = v.to_ascii_lowercase(),
            _ => {}
        }
    }
    if url.is_empty() {
        return None;
    }
    if token.is_empty() && !token_env.is_empty() {
        token = std::env::var(&token_env).unwrap_or_default();
    }
    Some(crate::voice_ws::VoiceCfg {
        url,
        token: (!token.is_empty()).then_some(token),
        mirror,
        reply,
    })
}

pub fn voice_start_place(_place: &Place) -> Result<()> {
    if crate::voice_ws::configured() {
        return crate::voice_ws::start();
    }
    crate::voice::start()
}

pub fn voice_stop_place(_place: &Place) -> Result<String> {
    if crate::voice_ws::configured() {
        return crate::voice_ws::stop();
    }
    crate::voice::stop()
}

/// Latest partial transcript for a take that is still running.
pub fn voice_peek_place(_place: &Place) -> Result<String> {
    if crate::voice_ws::configured() {
        return crate::voice_ws::peek();
    }
    crate::voice::peek()
}

/// Press the composer's mic: start capturing this Mac's microphone.
pub fn voice_start(_workspace: &Path) -> Result<()> {
    crate::voice::start()
}

/// Release the mic: return whatever was transcribed.
pub fn voice_stop(_workspace: &Path) -> Result<String> {
    crate::voice::stop()
}

/// `runtime/kernel.json` since the file-system design's Phase 1; the old
/// root location while kernels from before it are still around.
fn kernel_json(workspace: &Path) -> PathBuf {
    let new = workspace.join(".arbos").join("runtime").join("kernel.json");
    if new.exists() {
        return new;
    }
    workspace.join(".arbos").join("kernel.json")
}

fn gateway_json(workspace: &Path) -> PathBuf {
    workspace.join(".arbos").join("web.json")
}

fn read_info(workspace: &Path) -> Option<WebInfo> {
    read_json_info(&kernel_json(workspace))
}

fn read_json_info(path: &Path) -> Option<WebInfo> {
    let body = std::fs::read(path).ok()?;
    serde_json::from_slice(&body).ok()
}

fn http_url(info: &WebInfo) -> Option<String> {
    let url = info.url.trim().trim_end_matches('/');
    url.starts_with("http://")
        .then(|| url.to_string())
        .or_else(|| url.starts_with("https://").then(|| url.to_string()))
}

/// Whether the process answers on the attach port. `kernel.json` url is
/// `tcp://127.0.0.1:port`.
fn alive(info: &WebInfo) -> bool {
    let Some(addr) = tcp_addr(&info.url) else {
        return false;
    };
    std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
}

pub fn tcp_addr(url: &str) -> Option<std::net::SocketAddr> {
    let raw = url
        .trim()
        .trim_start_matches("tcp://")
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or("");
    raw.parse().ok()
}

fn spawn(workspace: &Path) -> Result<Child> {
    let bin = arbos_bin()?;
    // The kernel's stdout/stderr go under runtime/: process facts, never
    // part of the .arbos/ record.
    let dir = workspace.join(".arbos").join("runtime");
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let log = std::fs::File::create(dir.join("kernel.out.log"))
        .with_context(|| format!("create {}/kernel.out.log", dir.display()))?;
    let err = log.try_clone()?;
    // The kernel picks its own loopback port and writes it to kernel.json
    // as `tcp://127.0.0.1:port`. Do not invent an HTTP URL here — that
    // port is not the attach port, and waiting on it is a 60s miss.
    let mut cmd = Command::new(&bin);
    cmd.arg("serve")
        .arg(workspace)
        .current_dir(workspace)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(err));
    cmd.spawn()
        .with_context(|| format!("failed to start {}", bin.display()))
}

fn wait_ready(workspace: &Path, mut child: Child) -> Result<WebInfo> {
    let deadline = Instant::now() + READY_WAIT;
    while Instant::now() < deadline {
        if let Some(info) = read_info(workspace).filter(alive) {
            return Ok(info);
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(anyhow!(
                "arbos-kernel exited ({status}){}",
                kernel_log_tail(workspace)
            ));
        }
        thread::sleep(POLL);
    }
    let _ = child.kill();
    Err(anyhow!(
        "arbos-kernel did not write a live .arbos/kernel.json within {:?}{}",
        READY_WAIT,
        kernel_log_tail(workspace)
    ))
}

fn kernel_log_tail(workspace: &Path) -> String {
    let path = workspace.join(".arbos").join("runtime").join("kernel.out.log");
    let Ok(body) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let tail: String = body
        .lines()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    if tail.trim().is_empty() {
        String::new()
    } else {
        format!(": {tail}")
    }
}

fn arbos_bin() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("ARBOS_KERNEL_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(anyhow!(
            "ARBOS_KERNEL_BIN is not a file: {}",
            path.display()
        ));
    }
    if let Ok(path) = std::env::var("CARGO_MANIFEST_DIR") {
        let debug = PathBuf::from(path).join("../target/debug/arbos-kernel");
        if debug.is_file() {
            return Ok(debug.canonicalize()?);
        }
    }
    let local = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/debug/arbos-kernel");
    if local.is_file() {
        return Ok(local.canonicalize()?);
    }
    if let Some(home) = dirs::home_dir() {
        let cargo = home.join(".cargo").join("bin").join("arbos-kernel");
        if cargo.is_file() {
            return Ok(cargo);
        }
    }
    Ok(PathBuf::from("arbos-kernel"))
}

// ── remote ───────────────────────────────────────────────────────────

struct Probe {
    arch: String,
    has_bin: bool,
    /// The remote kernel's `--version` line, when it is one of ours.
    version: Option<String>,
    running: Option<WebInfo>,
}

fn attach_remote(host: &str, path: &Path) -> Result<WebInfo> {
    let key = Place::remote(host, path).encode();
    attach_remote_cached(&key, || open_remote_tunnel(host, path))
}

fn attach_remote_cached(key: &str, create: impl FnOnce() -> Result<Tunnel>) -> Result<WebInfo> {
    static ATTACH_LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    let lock = ATTACH_LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .entry(key.to_owned())
        .or_default()
        .clone();
    let _attach = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let cached = {
        let map = tunnels_lock();
        map.get(key).map(|t| t.info.clone())
    };
    if let Some(info) = cached.filter(|info| alive(info)) {
        return Ok(info);
    }

    let tunnel = create()?;
    let info = tunnel.info.clone();
    tunnels_lock().insert(key.to_owned(), tunnel);
    Ok(info)
}

fn open_remote_tunnel(host_name: &str, path: &Path) -> Result<Tunnel> {
    let target = remote_target(host_name);
    let host = target.ssh.as_str();
    let mut probe = ssh_probe(&target, path)?;
    // No binary, or one that is not this window's version and no kernel
    // running from it: put ours there (same machine type), so a place never
    // runs a kernel older than the window that opens it.
    let stale = probe.has_bin
        && probe.running.is_none()
        && probe.version.as_deref() != Some(local_kernel_version().as_str());
    if !probe.has_bin || stale {
        ssh_install_kernel(&target, &probe.arch, stale)?;
        probe = ssh_probe(&target, path)?;
        if !probe.has_bin {
            return Err(anyhow!(
                "could not install arbos-kernel on {} ({})",
                target.name,
                target.bin
            ));
        }
    }

    let remote_port = if let Some(info) = probe.running {
        port_of(&info.url).ok_or_else(|| anyhow!("arbos on {host} announced no port"))?
    } else {
        let port = random_port();
        ssh_launch(&target, path)?;
        let info = wait_remote_json(host, path)?;
        port_of(&info.url).unwrap_or(port)
    };

    let local_port = stable_local_port(host, path, "tcp");
    let mut forwards = vec![(local_port, remote_port)];
    let http = match ssh_gateway_info(host, path).and_then(|gw| port_of(&gw.url)) {
        Some(remote_http) if remote_http == remote_port => {
            Some(format!("http://127.0.0.1:{local_port}"))
        }
        Some(remote_http) => {
            let local_http = stable_local_port(host, path, "http");
            forwards.push((local_http, remote_http));
            Some(format!("http://127.0.0.1:{local_http}"))
        }
        None => None,
    };
    let child = ssh_tunnel(host, &forwards)?;
    let info = WebInfo {
        url: format!("tcp://127.0.0.1:{local_port}"),
        pid: 0,
        started: 0,
    };
    let tunnel = Tunnel { child, info, http };
    wait_alive(&tunnel.info)
        .with_context(|| format!("arbos on {host} did not answer through the tunnel"))?;
    Ok(tunnel)
}

/// Live HTTP gateway on the host (`web.json`), if its pid still answers.
fn ssh_gateway_info(host: &str, path: &Path) -> Option<WebInfo> {
    let dir = shell_path(&path.to_string_lossy());
    let script = format!(
        r#"f={dir}/.arbos/web.json
if [ -f "$f" ]; then pid=$(sed -n 's/.*"pid":\([0-9]*\).*/\1/p' "$f"); if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then cat "$f"; fi; fi"#,
        dir = dir,
    );
    let out = ssh_run(host, &script).ok()?;
    if out.status != 0 {
        return None;
    }
    let text = out.stdout.trim();
    if text.is_empty() {
        return None;
    }
    serde_json::from_str(text).ok()
}

fn ssh_probe(target: &RemoteTarget, path: &Path) -> Result<Probe> {
    let host = target.ssh.as_str();
    let dir = shell_path(&path.to_string_lossy());
    let script = format!(
        r#"os=$(uname -s | tr A-Z a-z); a=$(uname -m); case "$a" in x86_64|amd64) a=amd64;; aarch64|arm64) a=arm64;; esac; echo "$os-$a"
if [ -x "{bin}" ]; then sha=$(sha256sum "{bin}" 2>/dev/null | cut -d" " -f1); ver=$("{bin}" --version 2>/dev/null | head -n1 || echo -); else sha=-; ver=-; fi
echo "$sha"; echo "${{ver:--}}"
f={dir}/.arbos/runtime/kernel.json; [ -f "$f" ] || f={dir}/.arbos/kernel.json
if [ -f "$f" ]; then pid=$(sed -n 's/.*"pid":\([0-9]*\).*/\1/p' "$f"); if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then cat "$f"; fi; fi"#,
        bin = target.bin,
        dir = dir,
    );
    let out = ssh_run(host, &script)?;
    if out.status != 0 {
        return Err(anyhow!("{}", out.problem()));
    }
    let lines: Vec<&str> = out.stdout.lines().map(str::trim).collect();
    if lines.len() < 3 {
        return Err(anyhow!("unexpected probe output from {host}"));
    }
    let arch = lines[0].to_string();
    let has_bin = lines[1] != "-";
    // `arbos-kernel --version` prints `arbos-kernel 0.2.0 <sha> protocol 1`;
    // an older kernel answers with an unknown-command error, which is as
    // good as "not ours".
    let version = has_bin
        .then(|| lines[2].to_string())
        .filter(|v| v.starts_with("arbos-kernel "));
    let running = (lines.len() >= 4)
        .then(|| serde_json::from_str::<WebInfo>(&lines[3..].join("\n")).ok())
        .flatten();
    Ok(Probe {
        arch,
        has_bin,
        version,
        running,
    })
}

/// What this window's own kernel says for `--version`, so a remote copy
/// can be compared to it by the same string.
fn local_kernel_version() -> String {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION
        .get_or_init(|| {
            arbos_bin()
                .ok()
                .and_then(|bin| Command::new(bin).arg("--version").output().ok())
                .map(|out| {
                    String::from_utf8_lossy(&out.stdout)
                        .lines()
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_string()
                })
                .unwrap_or_default()
        })
        .clone()
}

/// Put `arbos-kernel` on the host at the target's path. Same machine
/// type: copy this window's binary (also over a stale one). Different
/// type: build from source only when the machine allows it in
/// `machines.toml` (`build = true`); otherwise say where a binary must go.
fn ssh_install_kernel(target: &RemoteTarget, remote_arch: &str, replacing: bool) -> Result<()> {
    let host = target.ssh.as_str();
    let bin_dir = parent_of(&target.bin);
    let mkdir = ssh_run(
        host,
        &format!(
            r#"umask 077 && mkdir -p "{bin_dir}" "$HOME/.cache/arbos""#,
            bin_dir = bin_dir
        ),
    )?;
    if mkdir.status != 0 {
        return Err(anyhow!("mkdir on {host}: {}", mkdir.problem()));
    }

    if local_os_arch() == remote_arch {
        let bin = arbos_bin().context("local arbos-kernel")?;
        if bin.is_file() {
            // Into a temp name first, then moved: a kernel that is being
            // executed must not be overwritten in place.
            let tmp = format!("{}.new", target.bin);
            ssh_put(host, &bin, &tmp)?;
            let swap = ssh_run(
                host,
                &format!(
                    r#"chmod +x "{tmp}" && mv -f "{tmp}" "{bin}""#,
                    tmp = tmp,
                    bin = target.bin
                ),
            )?;
            if swap.status == 0 {
                return Ok(());
            }
            return Err(anyhow!(
                "could not place arbos-kernel at {} on {}: {}",
                target.bin,
                target.name,
                swap.problem()
            ));
        }
    }
    if replacing {
        // A stale kernel of another architecture: leave it, say so.
        return Err(anyhow!(
            "arbos-kernel on {name} ({bin}) is not this window's version and cannot be replaced from here ({here} vs {there}); update it there, or set build = true for {name} in machines.toml",
            name = target.name,
            bin = target.bin,
            here = local_os_arch(),
            there = remote_arch
        ));
    }
    if !target.build {
        return Err(anyhow!(
            "no arbos-kernel on {name} and this window is {here}, the machine {there}: put an arbos-kernel built for it at {bin}, or set build = true for {name} in ~/.config/arbos/machines.toml to build from source there (needs cargo and a C compiler)",
            name = target.name,
            bin = target.bin,
            here = local_os_arch(),
            there = remote_arch
        ));
    }

    ssh_sync_kernel_src(host)?;

    let script = format!(
        r#"set -e
if ! command -v cargo >/dev/null 2>&1; then
  if [ ! -x "$HOME/.cargo/bin/cargo" ]; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain nightly
  fi
  . "$HOME/.cargo/env"
fi
cd "$HOME/.cache/arbos/src"
cargo build --release -p arbos-kernel
mkdir -p "{bin_dir}"
cp target/release/arbos-kernel "{bin}"
test -x "{bin}"
"#,
        bin_dir = bin_dir,
        bin = target.bin
    );
    let out = ssh_run(host, &script)?;
    if out.status != 0 {
        return Err(anyhow!("build arbos-kernel on {host}: {}", out.problem()));
    }
    Ok(())
}

/// The directory part of a remote path, kept as the shell will expand it.
fn parent_of(path: &str) -> String {
    match path.rfind('/') {
        Some(0) => "/".to_string(),
        Some(ix) => path[..ix].to_string(),
        None => ".".to_string(),
    }
}

fn ssh_sync_kernel_src(host: &str) -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let root = root
        .canonicalize()
        .with_context(|| format!("kernel source {}", root.display()))?;
    for need in [
        "Cargo.toml",
        "crates/arbos-kernel/Cargo.toml",
        "vendor/tgrep/tgrep-core/Cargo.toml",
    ] {
        if !root.join(need).is_file() {
            return Err(anyhow!(
                "cannot copy kernel source: missing {}",
                root.join(need).display()
            ));
        }
    }
    remember_mux_host(host);
    let mut tar = Command::new("tar")
        .current_dir(&root)
        .args([
            "czf",
            "-",
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain.toml",
            "crates/arbos-core",
            "crates/arbos-engine",
            "crates/arbos-kernel",
            "vendor/tgrep",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("tar kernel source")?;
    let stdout = tar.stdout.take().context("tar stdout")?;
    let unpack = Command::new("ssh")
        .args(ssh_shared())
        .arg(host)
        .arg(r#"mkdir -p "$HOME/.cache/arbos/src" && tar xzf - -C "$HOME/.cache/arbos/src""#)
        .stdin(Stdio::from(stdout))
        .output()
        .context("ssh unpack kernel source")?;
    let tar_status = tar.wait().context("tar wait")?;
    if !tar_status.success() {
        return Err(anyhow!("tar kernel source failed"));
    }
    if !unpack.status.success() {
        return Err(anyhow!(
            "copy kernel source to {host}: {}",
            String::from_utf8_lossy(&unpack.stderr)
                .lines()
                .next_back()
                .unwrap_or("ssh failed")
        ));
    }
    // Root Cargo.toml patches bezel-markdown to desktop/vendor. That crate
    // is not the kernel and is not in this tarball — cargo then dies.
    let clean = kernel_workspace_toml(&root)?;
    let tmp = std::env::temp_dir().join("arbos-kernel-workspace.toml");
    std::fs::write(&tmp, clean).context("write kernel workspace toml")?;
    ssh_put(host, &tmp, ".cache/arbos/src/Cargo.toml")?;
    let _ = std::fs::remove_file(&tmp);
    Ok(())
}

fn kernel_workspace_toml(root: &Path) -> Result<String> {
    let text = std::fs::read_to_string(root.join("Cargo.toml"))
        .with_context(|| format!("read {}", root.join("Cargo.toml").display()))?;
    let cut = text.find("[patch.").unwrap_or(text.len());
    Ok(format!("{}\n", text[..cut].trim_end()))
}

fn ssh_put(host: &str, local: &Path, remote: &str) -> Result<()> {
    remember_mux_host(host);
    let dest = format!("{host}:{remote}");
    let status = Command::new("scp")
        .args(ssh_shared())
        .arg("-q")
        .arg(local)
        .arg(&dest)
        .status()
        .context("scp")?;
    if !status.success() {
        return Err(anyhow!("scp {} to {host} failed", local.display()));
    }
    Ok(())
}

fn local_os_arch() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };
    format!("{os}-{arch}")
}

fn ssh_launch(target: &RemoteTarget, path: &Path) -> Result<()> {
    let host = target.ssh.as_str();
    let dir = shell_path(&path.to_string_lossy());
    // A machine from machines.toml keeps its kernels' config inside its
    // own directory (`<dir>/config`), the way `spawn host=` does.
    let env = match &target.config_home {
        Some(home) => format!("XDG_CONFIG_HOME={} ", shell_path(home)),
        None => String::new(),
    };
    let launch = format!(
        "cd {dir} && {env}exec {bin} serve {dir}",
        dir = dir,
        env = env,
        bin = target.bin,
    );
    let inner = launch.replace('\'', "'\\''");
    let script = format!(
        r#"umask 077 && mkdir -p "$HOME/.arbos"
if command -v setsid >/dev/null 2>&1; then
  setsid nohup sh -c '{inner}' >>"$HOME/.arbos/web.log" 2>&1 </dev/null &
else
  nohup sh -c '{inner}' >>"$HOME/.arbos/web.log" 2>&1 </dev/null &
fi
echo started"#
    );
    let out = ssh_run(host, &script)?;
    if out.status != 0 {
        return Err(anyhow!("{}", out.problem()));
    }
    Ok(())
}

fn wait_remote_json(host: &str, path: &Path) -> Result<WebInfo> {
    let dir = shell_path(&path.to_string_lossy());
    let deadline = Instant::now() + READY_WAIT;
    while Instant::now() < deadline {
        let out = ssh_run(
            host,
            &format!("cat {dir}/.arbos/runtime/kernel.json 2>/dev/null || cat {dir}/.arbos/kernel.json 2>/dev/null"),
        )?;
        if out.status == 0
            && let Ok(info) = serde_json::from_str::<WebInfo>(&out.stdout)
        {
            return Ok(info);
        }
        thread::sleep(Duration::from_millis(500));
    }
    Err(anyhow!(
        "arbos did not start within 60 s (see ~/.arbos/web.log on {host})"
    ))
}

fn ssh_tunnel(host: &str, forwards: &[(u16, u16)]) -> Result<Child> {
    let mut cmd = Command::new("ssh");
    cmd.args(ssh_base())
        .args(["-N", "-o", "ExitOnForwardFailure=yes"]);
    for (local, remote) in forwards {
        cmd.args(["-L", &format!("{local}:127.0.0.1:{remote}")]);
    }
    let mut child = cmd
        .arg(host)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("could not run ssh")?;
    thread::sleep(Duration::from_millis(200));
    if let Ok(Some(status)) = child.try_wait() {
        return Err(anyhow!("ssh tunnel to {host} exited ({status})"));
    }
    Ok(child)
}

fn wait_alive(info: &WebInfo) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if alive(info) {
            return Ok(());
        }
        thread::sleep(POLL);
    }
    Err(anyhow!("kernel did not answer at {}", info.url))
}

struct SshOut {
    status: i32,
    stdout: String,
    stderr: String,
}

impl SshOut {
    fn problem(&self) -> String {
        self.stderr
            .lines()
            .map(str::trim)
            .filter(|line| {
                !line.is_empty()
                    && !line.starts_with("** ")
                    && !line.starts_with("Warning: Permanently added")
            })
            .next_back()
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("ssh exited with status {}", self.status))
    }
}

/// Visible directory names under `path` on `host`. Hidden names (`.` prefix),
/// files, `.`, and `..` stay off the wire. Cap keeps a huge home from stalling.
const REMOTE_DIR_CAP: usize = 200;

pub fn list_remote_dirs(host: &str, path: &str) -> Result<Vec<String>> {
    // `ls -1p` is one level, no hidden (`-A`/`-a` omitted). `grep '/$'` keeps
    // directories only, so files never cross SSH. `head` caps the payload.
    let script = format!(
        "cd {} && {{ ls -1p 2>/dev/null | grep '/$' || true; }} | head -n {REMOTE_DIR_CAP}",
        shell_path(path)
    );
    let out = ssh_run_with(host, &script, &ssh_listing())?;
    if out.status != 0 {
        return Err(anyhow!(out.problem()));
    }
    let mut names: Vec<String> = out
        .stdout
        .lines()
        .filter_map(|line| {
            let name = line.trim().trim_end_matches('/');
            if name.is_empty() || name == "." || name == ".." || name.starts_with('.') {
                return None;
            }
            Some(name.to_string())
        })
        .take(REMOTE_DIR_CAP)
        .collect();
    names.sort_unstable();
    Ok(names)
}

/// Listing mux stays up for a minute so backspace / the next folder is a
/// reused hop, not a new handshake. Tunnel attach still uses `ssh_shared`.
fn ssh_listing() -> Vec<String> {
    let dir = ssh_control_dir();
    let control = format!("ControlPath={}/{}", dir.display(), "%C");
    let mut args = ssh_base();
    args.extend([
        "-o".into(),
        "ControlMaster=auto".into(),
        "-o".into(),
        control,
        "-o".into(),
        "ControlPersist=60".into(),
    ]);
    args
}

fn ssh_run(host: &str, command: &str) -> Result<SshOut> {
    ssh_run_with(host, command, &ssh_shared())
}

/// Same mux, 2s connect — delete must not sit on a dead hop.
fn ssh_run_brief(host: &str, command: &str) -> Result<SshOut> {
    let dir = ssh_control_dir();
    let control = format!("ControlPath={}/{}", dir.display(), "%C");
    let args = [
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=2",
        "-o",
        "StrictHostKeyChecking=accept-new",
        "-o",
        "ControlMaster=auto",
        "-o",
        control.as_str(),
        "-o",
        "ControlPersist=no",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();
    ssh_run_with(host, command, &args)
}

fn ssh_run_with(host: &str, command: &str, args: &[String]) -> Result<SshOut> {
    remember_mux_host(host);
    let output = Command::new("ssh")
        .args(args)
        .arg(host)
        .arg(command)
        .stdin(Stdio::null())
        .output()
        .context("could not run ssh")?;
    Ok(SshOut {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn ssh_base() -> Vec<String> {
    [
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=8",
        "-o",
        "StrictHostKeyChecking=accept-new",
        "-o",
        "ServerAliveInterval=15",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn ssh_shared() -> Vec<String> {
    let dir = ssh_control_dir();
    let control = format!("ControlPath={}/{}", dir.display(), "%C");
    let mut args = ssh_base();
    args.extend([
        "-o".into(),
        "ControlMaster=auto".into(),
        "-o".into(),
        control,
        "-o".into(),
        "ControlPersist=no".into(),
    ]);
    args
}

fn ssh_control_dir() -> PathBuf {
    let dir = settings::dir()
        .unwrap_or_else(|_| PathBuf::from("/tmp/arbos-desktop"))
        .join("ssh");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn shell_path(path: &str) -> String {
    if path == "~" {
        return "~".into();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return format!("~/{}", shell_quote(rest));
    }
    shell_quote(path)
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn port_of(url: &str) -> Option<u16> {
    url.rsplit_once(':')?.1.trim_end_matches('/').parse().ok()
}

fn random_port() -> u16 {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    REMOTE_PORTS.0 + (n as u16 % (REMOTE_PORTS.1 - REMOTE_PORTS.0))
}

fn stable_local_port(host: &str, path: &Path, salt: &str) -> u16 {
    let mut h: u64 = 14695981039346656037;
    for b in host
        .as_bytes()
        .iter()
        .chain(path.to_string_lossy().as_bytes())
        .chain(salt.as_bytes())
    {
        h ^= u64::from(*b);
        h = h.wrapping_mul(1099511628211);
    }
    let start = REMOTE_PORTS.0 + (h % u64::from(REMOTE_PORTS.1 - REMOTE_PORTS.0)) as u16;
    for port in start..REMOTE_PORTS.1 {
        if port_free(port) {
            return port;
        }
    }
    for port in REMOTE_PORTS.0..start {
        if port_free(port) {
            return port;
        }
    }
    start
}

fn port_free(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}
