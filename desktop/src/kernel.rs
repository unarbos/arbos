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
/// Where a host that is not in `machines.toml` gets its kernel. A directory
/// of the app's own, never `~/.cargo/bin`: that path is the box's shared
/// default on a person's `PATH`, and replacing the file there restarts only
/// the process that came for it — Jacob's own project on ArbosLife was left
/// running a deleted binary three times in forty hours by kernels installed
/// into it (mesh sweep, `internal/mesh-stale-binary-sweep-2026-09-17.md`).
const REMOTE_BIN: &str = "$HOME/.arbos-remote/bin/arbos-kernel";

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
/// What a running kernel's own gate says about being restarted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// Nothing is running in it. Replacing it loses nothing.
    Idle,
    /// Something is, and this is what: a turn, a parked approval, a detached
    /// job, a remote child. The kernel's words, not ours.
    Busy(String),
    /// It did not say — too old to carry `update_gate`, or it would not
    /// answer. Treated as busy: not knowing is not permission.
    Unknown,
}

impl Gate {
    pub fn idle(&self) -> bool {
        matches!(self, Self::Idle)
    }

    pub fn say(&self) -> String {
        match self {
            Self::Idle => "nothing is running in it".into(),
            Self::Busy(why) => why.clone(),
            Self::Unknown => "it is too old to say whether it is busy".into(),
        }
    }
}

/// Why a running kernel is not one this app should be talking to.
///
/// Three of them, because they are three different problems and saying the
/// wrong one is its own harm: a person told "another build" who is actually
/// looking at a deleted file will go looking for the wrong thing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// The file it is executing has been replaced or removed underneath it.
    /// **On its own, whatever the commits say** — a kernel still serving a
    /// deleted image of the *same* build is exactly as stale as one from
    /// another build, and it is what happened on 2026-09-17: the app updated
    /// in place, the commits matched, the bar stayed quiet, five workers hung.
    BinaryGone { built_at: Option<String> },
    /// Its commit is not the one this app ships.
    DifferentBuild { running: String, bundled: String },
    /// It could not say what it was built from. Older kernels did not record
    /// a commit, and a build that cannot account for itself is not one to
    /// assume is current.
    UnknownBuild { bundled: String },
}

impl Reason {
    /// The three or four words on the plate. What is wrong, not that
    /// something is.
    pub fn headline(&self) -> &'static str {
        match self {
            Self::BinaryGone { .. } => "Kernel running a deleted build",
            Self::DifferentBuild { .. } | Self::UnknownBuild { .. } => "Kernel from another build",
        }
    }

    /// The sentence, for a log line or the first line of a tooltip.
    pub fn say(&self, place: &str) -> String {
        match self {
            Self::BinaryGone { built_at } => format!(
                "the kernel serving {place} was replaced on disk{} and is still running the \
                 old image",
                match built_at {
                    Some(at) => format!(" (it was built {at})"),
                    None => String::new(),
                }
            ),
            Self::DifferentBuild { running, bundled } => format!(
                "the kernel serving {place} was built from {running}, and this app ships {bundled}"
            ),
            // Not "was built from unknown", which reads as a commit called
            // unknown. It is a kernel that did not record one.
            Self::UnknownBuild { bundled } => format!(
                "the kernel serving {place} is from a build that did not record its commit, \
                 and this app ships {bundled}"
            ),
        }
    }
}

/// A kernel this app should not be quietly talking to.
#[derive(Debug, Clone)]
pub struct Skew {
    pub place: Place,
    pub reason: Reason,
    pub gate: Gate,
}

/// Which kernel answered, in the kernel's own words from its `hello` frame.
///
/// Read off the connection, which is the only thing that can answer it: the
/// binary this app would *launch* ([`arbos_bin`]) is a different fact, and the
/// two part company in exactly the case worth knowing about — a kernel that was
/// already running, from another build, when the app attached.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KernelBuild {
    /// Semver, as the kernel's own `CARGO_PKG_VERSION`.
    pub version: String,
    /// Short git sha. Empty, or the word `unknown`, from a build that recorded
    /// none — read it through [`Self::commit`] rather than printing it.
    pub git_sha: String,
    /// `YYYY-MM-DDTHH:MMZ`; empty from a build that recorded none.
    pub built_at: String,
    /// The file this kernel started from is gone — replaced or moved under it —
    /// so it runs an old image and a restart would run what is on disk now.
    /// The kernel reports this of itself; nothing here infers it.
    pub binary_gone: bool,
}

impl KernelBuild {
    /// The commit, where the build recorded one. `unknown` is a kernel saying
    /// it does not know, so it is `None` here rather than a string to print.
    pub fn commit(&self) -> Option<&str> {
        let sha = self.git_sha.trim();
        (!sha.is_empty() && sha != "unknown").then_some(sha)
    }

    /// Whether this is the kernel this app ships: `None` when that cannot be
    /// told rather than a guess either way — the bundle has not been read yet,
    /// or one of the two builds recorded no commit.
    pub fn is_the_bundled_build(&self) -> Option<bool> {
        let running = self.commit()?;
        match bundled_commit() {
            Bundled::Sha(bundled) => Some(arbos_update::kernel::same_commit(running, bundled)),
            Bundled::Unread | Bundled::Unreadable => None,
        }
    }
}

/// The commit of the kernel this app ships, asked once.
///
/// The bundled kernel is the one every place on this machine should be served
/// by; anything else is a survivor of an older bundle.
fn bundled_kernel_sha() -> Option<&'static str> {
    BUNDLED_SHA
        .get_or_init(|| {
            let bin = arbos_bin().ok()?;
            arbos_update::kernel::Running::read(&bin)
                .ok()
                .map(|k| k.sha)
        })
        .as_deref()
}

static BUNDLED_SHA: OnceLock<Option<String>> = OnceLock::new();

/// What is known about the bundled kernel's commit *without* reading it.
/// Reading runs the binary, which is not something a paint may do, so a view
/// asks this and [`warm_bundled_commit`] does the reading off the window's
/// thread. Three answers, and `Unread` is one of them.
pub enum Bundled {
    /// Nobody has read it yet. Not the same as unreadable.
    Unread,
    /// Read, and the binary could not say: none on this machine, or one too
    /// old to report a commit.
    Unreadable,
    Sha(&'static str),
}

pub fn bundled_commit() -> Bundled {
    match BUNDLED_SHA.get() {
        None => Bundled::Unread,
        Some(None) => Bundled::Unreadable,
        Some(Some(sha)) => Bundled::Sha(sha.as_str()),
    }
}

/// Read the bundled kernel's commit if nobody has. Runs the binary, so call it
/// from a background task.
pub fn warm_bundled_commit() {
    let _ = bundled_kernel_sha();
}

/// Whether the kernel described by `info` is one to warn about, and why.
fn skew(workspace: &Path, info: &WebInfo) -> Option<Skew> {
    let bundled = bundled_kernel_sha()?;
    let health = health_of(info);
    let running = read_info_sha(workspace);

    // Order matters. A deleted image is the most specific thing wrong and the
    // most urgent, and it is true whatever the commits say.
    let reason = if health.binary_gone {
        Reason::BinaryGone {
            built_at: health.built_at.clone(),
        }
    } else {
        match running {
            // A kernel from before commits were recorded. It used to fall out
            // of this function as "nothing to say", which left the one machine
            // most likely to be stale showing nothing at all.
            None => Reason::UnknownBuild {
                bundled: bundled.to_owned(),
            },
            Some(running) if !arbos_update::kernel::same_commit(&running, bundled) => {
                Reason::DifferentBuild {
                    running,
                    bundled: bundled.to_owned(),
                }
            }
            Some(_) => return None,
        }
    };
    Some(Skew {
        place: Place::local(workspace),
        reason,
        gate: health.gate,
    })
}

/// The `git_sha` a kernel wrote about itself when it started, where it said
/// one. `unknown` is not a commit; it is a kernel saying it does not know.
fn read_info_sha(workspace: &Path) -> Option<String> {
    let place = arbos_core::Place::new(workspace.to_path_buf());
    let text = std::fs::read_to_string(place.kernel_json_read()).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("git_sha")
        .and_then(|v| v.as_str())
        .filter(|sha| !sha.is_empty() && *sha != "unknown")
        .map(str::to_owned)
}

/// What a kernel says about itself on `/healthz`.
struct Health {
    /// Whether it may be restarted, in its own words.
    gate: Gate,
    /// Whether the file it is executing is gone. Absent from kernels older
    /// than the flag, where it reads false — they are covered by the commit
    /// comparison instead.
    binary_gone: bool,
    built_at: Option<String>,
}

/// One request, because two reads of the same document should not be two trips
/// to the same socket.
///
/// `update_gate` is the same verdict the self-updater uses, so the app and the
/// kernel agree about what "safe to restart" means rather than the app
/// guessing.
fn health_of(info: &WebInfo) -> Health {
    let quiet = Health {
        gate: Gate::Unknown,
        binary_gone: false,
        built_at: None,
    };
    let Some(addr) = tcp_addr(&info.url) else {
        return quiet;
    };
    let Ok(response) = http()
        .get(&format!("http://{addr}/healthz"))
        .call()
        .and_then(|mut r| r.body_mut().read_to_string().map_err(Into::into))
    else {
        return quiet;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&response) else {
        return quiet;
    };
    Health {
        gate: match json.get("update_gate") {
            None => Gate::Unknown,
            Some(gate) => match gate.get("verdict").and_then(|v| v.as_str()) {
                Some("idle") => Gate::Idle,
                Some("busy") => Gate::Busy(
                    gate.get("reason")
                        .and_then(|r| r.as_str())
                        .unwrap_or("something is running in it")
                        .to_owned(),
                ),
                _ => Gate::Unknown,
            },
        },
        binary_gone: json
            .get("binary_gone")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        built_at: json
            .get("built_at")
            .and_then(|v| v.as_str())
            .filter(|at| !at.is_empty())
            .map(str::to_owned),
    }
}

/// Stop one kernel by the pid it recorded, and wait for its port to go quiet.
fn stop_kernel(info: &WebInfo) {
    #[cfg(unix)]
    if info.pid > 0 {
        // SAFETY: a signal to a pid. SIGTERM is the kernel's graceful stop —
        // every running turn ends the way the stop button ends it.
        unsafe {
            libc::kill(info.pid, libc::SIGTERM);
        }
    }
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < until {
        if !alive(info) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Whether the kernel serving `place` is a stranger to this app, for the
/// window to show.
///
/// Read rather than acted on: the automatic case — a stranger with nothing
/// running in it — is already handled at attach. What is left here is the one
/// that needs a person, so this reports and the bar offers the choice.
pub fn kernel_skew(place: &Place) -> Option<Skew> {
    if place.is_remote() {
        return None;
    }
    let workspace = place.path.canonicalize().ok()?;
    let info = read_info(&workspace).filter(alive)?;
    skew(&workspace, &info)
}

/// Stop the kernel serving `place`, whatever it is running. The choice the
/// window offers when a stranger is busy.
pub fn restart_kernel(place: &Place) -> Result<()> {
    let workspace = place
        .path
        .canonicalize()
        .with_context(|| format!("not a directory: {}", place.path.display()))?;
    let info = read_info(&workspace)
        .filter(alive)
        .context("no kernel is running there")?;
    stop_kernel(&info);
    attach_or_spawn(&workspace).map(|_| ())
}

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
        // Is this kernel the one this app ships? After an update it may not
        // be: the app's own stop cannot reach every kernel on the machine, and
        // one that outlived a swap goes on serving from a binary that is not
        // there any more. On 2026-09-17 that kernel was 223 commits behind and
        // the app attached to it without a word, then sent frames it had never
        // heard of.
        match skew(&workspace, &info) {
            // Nobody is using it, so nothing is lost by replacing it with the
            // build this app came with. No question worth asking.
            Some(found) if found.gate.idle() => {
                eprintln!(
                    "arbos: {} — restarting it",
                    found.reason.say(&workspace.display().to_string())
                );
                stop_kernel(&info);
            }
            // Something is running in it. Attaching is still right — it is the
            // user's work and they must be able to watch it — but the window
            // has to say so, and offer the choice rather than take it. The bar
            // reads this back through `kernel_skew`.
            Some(found) => {
                eprintln!(
                    "arbos: {} — {}",
                    found.reason.say(&workspace.display().to_string()),
                    found.gate.say()
                );
                return Ok(info);
            }
            None => return Ok(info),
        }
    }
    // One spawn per place at a time. The chat, the board and the terminal
    // all attach when a place opens; without this, each of them started a
    // kernel, the losers of the `.arbos/runtime/lock` race exited 1, and
    // that exit landed in the transcript as a failed notice.
    let spawning = spawn_lock(&workspace);
    let _spawning = spawning
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(info) = read_info(&workspace).filter(alive) {
        return Ok(info);
    }
    let child = spawn(&workspace)?;
    wait_ready(&workspace, child)
}

fn spawn_lock(workspace: &Path) -> Arc<Mutex<()>> {
    static SPAWN_LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    SPAWN_LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .entry(workspace.to_path_buf())
        .or_default()
        .clone()
}

/// The kernel's own words when a second `serve` finds the place taken
/// (`arbos_core::PlaceLock::acquire`).
const LOCK_HELD: &str = "place already served";

/// A spawned kernel exited because another kernel holds the place: not an
/// error of ours, the other kernel is the one to attach to.
fn lost_lock_race(status: &std::process::ExitStatus, log_tail: &str) -> bool {
    !status.success() && log_tail.contains(LOCK_HELD)
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
    /// Whether the host lists image input for it (OpenRouter
    /// `architecture.input_modalities`). None: the host did not say; the
    /// name is the guess (`arbos_core::models::looks_vision`).
    pub vision: Option<bool>,
    /// A free endpoint (`:free`, or a zero price): OpenRouter's free
    /// providers are the ones whose terms commonly allow training on
    /// prompts, and the account's "free models" privacy toggle governs
    /// them separately. The picker says so.
    pub free: bool,
}

impl ModelOption {
    /// Takes image input, by the host's word or, failing that, by name.
    pub fn sees_images(&self) -> bool {
        self.vision
            .unwrap_or_else(|| arbos_core::models::looks_vision(&self.id))
    }
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
            free: row.id.ends_with(":free"),
            id: row.id,
            vision: None,
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
            vision: row
                .architecture
                .as_ref()
                .filter(|a| !a.input_modalities.is_empty())
                .map(|a| a.input_modalities.iter().any(|m| m == "image")),
            free: row.is_free(),
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
    /// The worker cannot write (a read-only kind, or `readonly: true` on
    /// the spawn): drawn as a glyph after its name, so a project whose
    /// workers can all only read looks wrong at a glance (F-56).
    pub readonly: bool,
    /// The definition the worker was spawned from (`explore`, …), when one
    /// was named.
    pub agent_kind: Option<String>,
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
                readonly: false,
                agent_kind: None,
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
    readonly=$(awk -F': *' '/^readonly:/ {{print $2; exit}}' "$p/agent.md")
    kind=$(awk -F': *' '/^kind:/ {{print $2; exit}}' "$p/agent.md")
  fi
  printf '%s\t%s\t%s\t%s\t%s\n' "$id" "$name" "$parent" "$readonly" "$kind"
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
                let mut parts = line.splitn(5, '\t');
                let id = parts.next()?.trim();
                let name = parts.next().unwrap_or("").trim();
                let parent = parts.next().unwrap_or("").trim();
                let readonly = parts.next().unwrap_or("").trim() == "true";
                let kind = parts.next().unwrap_or("").trim();
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
                    readonly,
                    agent_kind: (!kind.is_empty()).then(|| kind.to_string()),
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
            readonly: front.readonly,
            agent_kind: front.kind,
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
    readonly: bool,
    kind: Option<String>,
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
        } else if let Some(value) = line.strip_prefix("readonly:") {
            front.readonly = value.trim() == "true";
        } else if let Some(value) = line.strip_prefix("kind:") {
            front.kind = field(value);
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
                    step: 0,
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
    // A finished worker is moved to the archive with its transcript; read
    // back from there, or a worker's chat opened after a relaunch showed
    // its brief alone (F-133, cycle 32).
    let store = place.path.join(".arbos");
    let text = [
        store.join("agents").join(id).join("transcript.jsonl"),
        store
            .join("archive")
            .join("agents")
            .join(id)
            .join("transcript.jsonl"),
    ]
    .into_iter()
    .find_map(|path| std::fs::read_to_string(path).ok())?;
    let mut items = Vec::new();
    // Timestamps give the replay what the live view measures: the turn's
    // wall time on its prompt, and a thought's seconds as the gap to the
    // event after it.
    let mut turn_began: Option<i64> = None;
    let mut thinking_since: Option<i64> = None;
    // A `user` line while a turn is open (after its wake, before its
    // turn_complete) was a steer: its card stays inside that turn.
    let mut turn_open = false;
    // The kernel writes `wake user` and then the `user` line that caused
    // it: that line is the turn's prompt, not a steer into it. Read as a
    // steer, every prompt of a forked chat folded into the turn before it
    // and the answers vanished behind shut folds (Jacob, report
    // 2026-09-17-17, F-147).
    let mut prompt_pending = false;
    for line in text.lines() {
        if let Ok(ev) = serde_json::from_str::<arbos_core::Event>(line) {
            let thinking = matches!(ev.kind, arbos_core::EventKind::Thinking { .. });
            let is_user = matches!(ev.kind, arbos_core::EventKind::User { .. });
            let steer = turn_open && is_user && !prompt_pending;
            if is_user {
                prompt_pending = false;
            }
            match &ev.kind {
                arbos_core::EventKind::Wake { wake, .. } => {
                    turn_open = true;
                    prompt_pending = wake == "user";
                }
                arbos_core::EventKind::TurnComplete { .. }
                | arbos_core::EventKind::Interrupted { .. } => turn_open = false,
                _ => {}
            }
            if !thinking {
                if let Some(since) = thinking_since.take() {
                    // The record's own `secs` (settled thoughts since #221)
                    // beats the gap to the next event.
                    if let Some(crate::model::session::ChatItem::Thinking { secs, .. }) =
                        items.last_mut()
                        && secs.is_none()
                    {
                        *secs = secs_between(since, ev.ts);
                    }
                }
            }
            match &ev.kind {
                arbos_core::EventKind::User { .. } if !steer => turn_began = Some(ev.ts),
                // A wake of the kernel's own (a worker's report, a
                // subscription) opens a segment whose clock starts here. So
                // does a worker's `plan` wake: it is the brief's card below,
                // and the worker's first turn has no `user` line to start
                // the clock — read back, its headline lost its "Worked for
                // 20s" and showed the summary phrase instead (F-131).
                arbos_core::EventKind::Wake { wake, .. }
                    if !matches!(wake.as_str(), "user" | "kickoff" | "compact") =>
                {
                    turn_began = Some(ev.ts)
                }
                arbos_core::EventKind::Thinking { .. } => {
                    thinking_since.get_or_insert(ev.ts);
                }
                arbos_core::EventKind::TurnComplete { .. } => {
                    if let Some(began) = turn_began.take() {
                        match items.iter_mut().rev().find(|item| {
                            matches!(
                                item,
                                crate::model::session::ChatItem::User(_)
                                    | crate::model::session::ChatItem::Wake { .. }
                            )
                        }) {
                            Some(crate::model::session::ChatItem::User(message)) => {
                                message.worked_secs = secs_between(began, ev.ts);
                            }
                            Some(crate::model::session::ChatItem::Wake { secs, .. }) => {
                                *secs = secs_between(began, ev.ts);
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            if let Some(mut item) = event_to_item(&ev) {
                if steer && let crate::model::session::ChatItem::User(message) = &mut item {
                    message.steer = true;
                }
                // A wake written after the worker's `say` that caused it
                // opens the segment; the report reads under its header.
                if matches!(item, crate::model::session::ChatItem::Wake { .. })
                    && matches!(
                        items.last(),
                        Some(crate::model::session::ChatItem::From { .. })
                    )
                {
                    let report = items.pop();
                    items.push(item);
                    if let Some(report) = report {
                        items.push(report);
                    }
                    continue;
                }
                // A `status` call between two reasoning steps draws no row;
                // the thoughts on either side of it are one thought.
                let last_shown = items
                    .iter()
                    .rposition(|held| {
                        !matches!(held, crate::model::session::ChatItem::Tool { label, .. }
                            if crate::view::component::transcript::is_status_call(label))
                    })
                    .filter(|_| matches!(item, crate::model::session::ChatItem::Thinking { .. }));
                match (&item, last_shown.and_then(|at| items.get_mut(at))) {
                    (
                        crate::model::session::ChatItem::Thinking { text, secs, .. },
                        Some(crate::model::session::ChatItem::Thinking {
                            text: held,
                            done,
                            secs: held_secs,
                        }),
                    ) => {
                        held.push_str(text);
                        *done = true;
                        // Two settled steps in a row read as one thought, their
                        // seconds added.
                        if let Some(more) = secs {
                            *held_secs = Some(held_secs.unwrap_or(0).saturating_add(*more));
                        }
                    }
                    _ => items.push(item),
                }
            }
        }
    }
    // The kernel's "Waiting for your answer" was true while the ask was
    // parked; in a replay only the last one can still be. A `status: …`
    // assistant line is the step the worker line showed, not prose.
    let n = items.len();
    let mut ix = 0;
    items.retain(|item| {
        ix += 1;
        match item {
            crate::model::session::ChatItem::Notice {
                text,
                failed: false,
            } if text.trim() == "Waiting for your answer" && ix < n => false,
            crate::model::session::ChatItem::Agent(text) => {
                crate::model::session::status_line(text).is_none()
            }
            _ => true,
        }
    });
    dedupe_replay(&mut items, id);
    Some(crate::model::history::Replay {
        items,
        model: agent_model(&arbos_core::Place::new(&place.path), id),
    })
}

/// A paragraph once, as the live session shows it (`dedupe_settled`): a
/// step that repeats the turn's previous prose word for word goes, and so
/// does a `say` to the user the agent then wrote out as its reply.
fn dedupe_replay(items: &mut Vec<crate::model::session::ChatItem>, own: &str) {
    use crate::model::session::ChatItem;
    let mut drop = vec![false; items.len()];
    for ix in 0..items.len() {
        let ChatItem::Agent(text) = &items[ix] else {
            continue;
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        for at in (0..ix).rev() {
            if drop[at] {
                continue;
            }
            match &items[at] {
                ChatItem::User(_) => break,
                ChatItem::Agent(earlier) if earlier.trim() == text => {
                    drop[ix] = true;
                    break;
                }
                ChatItem::From {
                    who, text: said, ..
                } if said.trim() == text && (who == own || who.is_empty()) => {
                    drop[at] = true;
                    break;
                }
                _ => {}
            }
        }
    }
    // A retry line replaces the retry before it, as the live session does.
    for ix in 1..items.len() {
        if let (ChatItem::Notice { text: prev, .. }, ChatItem::Notice { text: next, .. }) =
            (&items[ix - 1], &items[ix])
            && crate::model::session::is_retry_line(prev)
            && crate::model::session::is_retry_line(next)
        {
            drop[ix - 1] = true;
        }
    }
    let mut ix = 0;
    items.retain(|_| {
        let keep = !drop[ix];
        ix += 1;
        keep
    });
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
/// `skill:` from a local agent's `agent.md`: the mode pinned to the chat.
pub fn agent_skill(place: &arbos_core::Place, id: &str) -> Option<String> {
    let agent = arbos_core::Agent::load(&place.agent_dir(id)).ok()?;
    agent.skill.filter(|s| !s.trim().is_empty())
}

/// The names of the skills a place offers, for the mode chip's list.
pub fn skill_names(place: &arbos_core::Place) -> Vec<String> {
    arbos_core::load_skills(place)
        .iter()
        .map(|s| s.name.clone())
        .collect()
}

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
            message.seq = (ev.seq > 0).then_some(ev.seq);
            Some(ChatItem::User(message))
        }
        // An empty line is a step boundary for the kernel's projection, not
        // something the model said.
        // The brief a worker was spawned with is its prompt: Cursor shows a
        // subagent's as the first card. A plan wake with no text (a timer,
        // a chore) is not a message.
        arbos_core::EventKind::Wake {
            wake,
            text: Some(text),
            brief,
        } if wake == "plan" && !text.trim().is_empty() => {
            let mut message =
                crate::model::attachment::UserMessage::from(wake_brief(text, brief.as_deref()));
            message.sent_at = (ev.ts > 0).then_some(ev.ts);
            Some(ChatItem::User(message))
        }
        // A worker's report or a subscription firing: a segment of its own
        // in the Project chat, as Cursor draws it (F-62).
        arbos_core::EventKind::Wake { wake, text, .. }
            if !matches!(wake.as_str(), "user" | "kickoff" | "compact" | "plan") =>
        {
            Some(ChatItem::Wake {
                kind: wake.clone(),
                text: text.clone(),
                at: (ev.ts > 0).then_some(ev.ts),
                secs: None,
            })
        }
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
        arbos_core::EventKind::Thinking { text, .. } if text.trim().is_empty() => None,
        arbos_core::EventKind::Thinking { text, secs, .. } => Some(ChatItem::Thinking {
            text: text.clone(),
            done: true,
            secs: secs.map(|s| s.min(u32::MAX as u64) as u32),
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
                desc: rec
                    .label
                    .clone()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty()),
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
    // The copy rewrites spawn records so the fork claims none of the
    // original's workers (a fork listed under itself looped the window).
    let core_place = arbos_core::Place::new(&place.path);
    let agent = arbos_core::files::fork_chat(&core_place, source_id)
        .with_context(|| format!("fork {source_id} in {}", place.path.display()))?;
    Ok(agent.id.to_string())
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
    #[serde(default)]
    pricing: Option<UpstreamPricing>,
    /// OpenRouter lists what each model accepts. Absent on other hosts.
    #[serde(default)]
    supported_parameters: Vec<String>,
    #[serde(default)]
    architecture: Option<UpstreamArchitecture>,
}

#[derive(Deserialize, Default)]
struct UpstreamPricing {
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    completion: String,
}

impl UpstreamModel {
    /// `:free` in the id, or a zero price for both prompt and completion.
    fn is_free(&self) -> bool {
        self.id.ends_with(":free")
            || self.pricing.as_ref().is_some_and(|p| {
                let zero = |s: &str| s.trim().parse::<f64>().is_ok_and(|v| v == 0.0);
                zero(&p.prompt) && zero(&p.completion)
            })
    }
}

#[derive(Deserialize, Default)]
struct UpstreamArchitecture {
    #[serde(default)]
    input_modalities: Vec<String>,
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
    let mut work_sound = String::new();
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
            "voice_work_sound" => work_sound = v.to_ascii_lowercase(),
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
        work_sound: match work_sound.as_str() {
            "off" | "none" | "false" | "0" => crate::voice_ws::WorkSound::Off,
            "ticks" | "tick" => crate::voice_ws::WorkSound::Ticks,
            _ => crate::voice_ws::WorkSound::Bed,
        },
        mirror,
        reply,
    })
}

/// The name a call gives the speech server for a place: `<machine>/<folder>`,
/// the hub's way of naming a kernel (`docs/arbos-mesh-design.md`). The machine is
/// this computer's name in `~/.config/arbos/hub.toml` (`machine = "mac"`), or the
/// ssh alias for a remote place. Without either, the folder's name alone: the
/// gateway then takes it for its own kernel.
pub fn hub_project_name(place: &Place) -> String {
    let target = call_target(place, "");
    match target.machine {
        Some(machine) if !machine.is_empty() && !target.project.is_empty() => {
            format!("{machine}/{}", target.project)
        }
        _ => target.project,
    }
}

/// What a call tells the gateway it is for: the open tab's folder path (what
/// binds the call), the machine the hub knows this computer as, the folder's
/// name, the ssh alias for a remote place, and the tab's label.
pub fn call_target(place: &Place, label: &str) -> crate::voice_ws::CallTarget {
    let folder = place
        .path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    let machine = match &place.host {
        Some(alias) => Some(alias.clone()),
        None => hub_machine_name(),
    }
    .filter(|m| !m.is_empty());
    // A local folder is sent as it really is on disk (symlinks resolved), so
    // the gateway's own place compares equal to it.
    let path = match &place.host {
        Some(_) => place.path.to_string_lossy().to_string(),
        None => std::fs::canonicalize(&place.path)
            .unwrap_or_else(|_| place.path.clone())
            .to_string_lossy()
            .to_string(),
    };
    crate::voice_ws::CallTarget {
        machine,
        project: folder,
        path,
        host: place.host.clone(),
        name: label.to_string(),
        context: Default::default(),
    }
}

/// `machine = "…"` from `~/.config/arbos/hub.toml`, when this computer is on a hub.
fn hub_machine_name() -> Option<String> {
    let text = std::fs::read_to_string(arbos_core::host_dir().join("hub.toml")).ok()?;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if k.trim() == "machine" {
            let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
            return (!v.is_empty()).then_some(v);
        }
    }
    None
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

/// Kernels this process has started. Tests read it to prove one place gets
/// one spawn however many attachers race.
static SPAWNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
fn spawn_count() -> usize {
    SPAWNS.load(std::sync::atomic::Ordering::SeqCst)
}

fn spawn(workspace: &Path) -> Result<Child> {
    let bin = arbos_bin()?;
    SPAWNS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
    // Not the window's whole environment: launched from a shell, that is
    // every `export` in the user's rc file, and the kernel would hand it
    // to every job. The allowlist, the configured model key, the vault
    // token, and what the place's secrets.toml reads — nothing else.
    let key_env = Host::peek()
        .map(|h| h.config.key_env())
        .unwrap_or_else(|_| "OPENROUTER_API_KEY".to_string());
    let mut cmd = Command::new(&bin);
    cmd.arg("serve")
        .arg(workspace)
        .current_dir(workspace)
        .env_clear()
        .envs(arbos_core::envsafe::kernel_env(workspace, &key_env))
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
            let tail = kernel_log_tail(workspace);
            if lost_lock_race(&status, &tail) {
                // Another process (a CLI `serve`, an older desktop) holds the
                // place. Wait for its kernel.json instead of reporting ours.
                return wait_other(workspace, deadline);
            }
            return Err(anyhow!("arbos-kernel exited ({status}){tail}"));
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

/// The lock holder is starting up (or already serving): poll for its live
/// kernel.json until `deadline`.
fn wait_other(workspace: &Path, deadline: Instant) -> Result<WebInfo> {
    while Instant::now() < deadline {
        if let Some(info) = read_info(workspace).filter(alive) {
            return Ok(info);
        }
        thread::sleep(POLL);
    }
    Err(anyhow!(
        "another arbos-kernel holds {} but never wrote a live .arbos/kernel.json",
        workspace.join(".arbos").display()
    ))
}

fn kernel_log_tail(workspace: &Path) -> String {
    let path = workspace
        .join(".arbos")
        .join("runtime")
        .join("kernel.out.log");
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

pub(crate) fn arbos_bin() -> Result<PathBuf> {
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
    // A shipped app carries its kernel: `Arbos.app/Contents/MacOS/arbos-kernel`,
    // signed with the bundle (desktop/Makefile). It wins over any development
    // tree so a copy dragged out of the DMG works on a Mac with no checkout.
    if let Some(beside) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("arbos-kernel")))
        .filter(|path| path.is_file())
    {
        return Ok(beside);
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

/// The step a remote place's connect is on, by place key, for the window
/// to draw ("Installing arbos-kernel 0.2.1…", "Updating 0.2.0 → 0.2.1…").
/// Set by `open_remote_tunnel` as it goes; `Ready` when the tunnel is up;
/// `Failed` with the step when it is not. Read with `remote_progress`.
fn progress_lock() -> &'static Mutex<HashMap<String, arbos_core::remote_kernel::Progress>> {
    static P: OnceLock<Mutex<HashMap<String, arbos_core::remote_kernel::Progress>>> =
        OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
}

fn set_progress(key: &str, step: arbos_core::remote_kernel::Progress) {
    eprintln!("remote {key}: {step}");
    progress_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(key.to_string(), step);
}

/// Where a remote place's connect stands, if one is or was under way.
pub fn remote_progress(host: &str, path: &Path) -> Option<arbos_core::remote_kernel::Progress> {
    progress_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&Place::remote(host, path).encode())
        .cloned()
}

fn open_remote_tunnel(host_name: &str, path: &Path) -> Result<Tunnel> {
    use arbos_core::remote_kernel::Progress;
    let key = Place::remote(host_name, path).encode();
    let step = |p: Progress| set_progress(&key, p);
    match open_remote_tunnel_steps(host_name, path, &step) {
        Ok(t) => {
            step(Progress::Ready);
            Ok(t)
        }
        Err(e) => {
            let at = remote_progress(host_name, path)
                .map(|p| p.to_string().trim_end_matches('…').to_string())
                .unwrap_or_else(|| "Connecting".into());
            step(Progress::Failed {
                step: at,
                why: format!("{e:#}"),
            });
            Err(e)
        }
    }
}

fn open_remote_tunnel_steps(
    host_name: &str,
    path: &Path,
    step: &dyn Fn(arbos_core::remote_kernel::Progress),
) -> Result<Tunnel> {
    use arbos_core::remote_kernel::{KernelVersion, Progress};
    let target = remote_target(host_name);
    let host = target.ssh.as_str();
    step(Progress::Probing);
    let mut probe = ssh_probe(&target, path)?;
    // The version rule: the remote's kernel is replaced when it is older
    // than this window's (semver), or the same version from another
    // build; a newer remote is left alone. A running older kernel is
    // stopped first — that process alone; its jobs go with it — so a
    // place never runs a kernel older than the window that opens it.
    let mine = KernelVersion::parse(&local_kernel_version());
    let theirs = probe.version.as_deref().and_then(KernelVersion::parse);
    let wants_update = match (&theirs, &mine) {
        (Some(t), Some(m)) => t.needs_update_to(m),
        // Ours is not a version we can read (a dev build with no line):
        // an unreadable remote is replaced, a readable one kept.
        (None, _) => probe.has_bin,
        (Some(_), None) => false,
    };
    if !probe.has_bin || wants_update {
        match (&theirs, &mine) {
            (Some(t), Some(m)) => step(Progress::Updating {
                from: t.short(),
                to: m.short(),
            }),
            _ => step(Progress::Installing {
                version: mine
                    .as_ref()
                    .map(|m| m.short())
                    .unwrap_or_else(|| "this build".into()),
                // What crosses the wire when it is this window's own
                // binary; a release download's size is the script's.
                bytes: (local_os_arch() == probe.arch)
                    .then(|| arbos_bin().ok())
                    .flatten()
                    .and_then(|bin| std::fs::metadata(bin).ok())
                    .map(|meta| meta.len())
                    .unwrap_or(0),
            }),
        }
        // Swap before stop. Nothing is stopped until the new binary is
        // already at the path, because a supervisor that relaunches into
        // the gap does it from the path — and a stop-then-swap ordering
        // hands it the build we are replacing. That process is stale from
        // birth and pins the place's lock against its own supervisor.
        //
        // A machine with no kernel yet has nothing to swap and nothing to
        // restart, so it takes the plain install.
        if probe.has_bin {
            ssh_bootstrap_kernel(&target, &probe.arch, path, step)?;
        } else {
            ssh_install_kernel(&target, &probe.arch, &target.bin, false, step)?;
        }
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
        step(Progress::Starting);
        let port = random_port();
        ssh_launch(&target, path)?;
        let info = wait_remote_json(host, path)?;
        port_of(&info.url).unwrap_or(port)
    };
    step(Progress::Connecting);

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

/// The connect step for any place on `host` that is under way, as the
/// line the window shows where "connecting…" would be. A view over
/// `remote_progress`, keyed `host:path`; nothing once the tunnel is up or
/// the attempt has failed (the failure is reported on its own).
pub fn connect_step(host: &str) -> Option<String> {
    use arbos_core::remote_kernel::Progress;
    let prefix = format!("{host}:");
    progress_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .filter(|(key, _)| key.starts_with(&prefix))
        .find_map(|(_, step)| match step {
            Progress::Ready | Progress::Failed { .. } => None,
            step => Some(step.to_string()),
        })
}

/// Live HTTP gateway on the host (`web.json`), if its pid still answers.
fn ssh_gateway_info(host: &str, path: &Path) -> Option<WebInfo> {
    let dir = shell_path(&path.to_string_lossy());
    let script = format!(
        r#"f={dir}/.arbos/web.json
if [ -f "$f" ]; then pid=$(tr -d '\n' < "$f" | sed -n 's/.*"pid":[[:space:]]*\([0-9][0-9]*\).*/\1/p' | head -n1); if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then cat "$f"; fi; fi"#,
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
if [ -f "$f" ]; then pid=$(tr -d '\n' < "$f" | sed -n 's/.*"pid":[[:space:]]*\([0-9][0-9]*\).*/\1/p' | head -n1); if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then cat "$f"; fi; fi"#,
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

/// Put `arbos-kernel` on the host at `dest`. Same machine type: copy this
/// window's binary (also over a stale one). Different type: build from
/// source only when the machine allows it in `machines.toml`
/// (`build = true`); otherwise say where a binary must go.
///
/// `dest` is the installation's own path only when the machine has no
/// kernel yet. When it has one, `dest` is a staging path and the swap is
/// [`ssh_bootstrap_kernel`]'s, so that nothing is stopped before the new
/// binary is in place.
fn ssh_install_kernel(
    target: &RemoteTarget,
    remote_arch: &str,
    dest: &str,
    replacing: bool,
    step: &dyn Fn(arbos_core::remote_kernel::Progress),
) -> Result<()> {
    let host = target.ssh.as_str();
    let bin_dir = parent_of(dest);
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
            let tmp = format!("{dest}.new");
            ssh_put(host, &bin, &tmp)?;
            let swap = ssh_run(
                host,
                &format!(r#"chmod +x "{tmp}" && mv -f "{tmp}" "{dest}""#),
            )?;
            if swap.status == 0 {
                return Ok(());
            }
            return Err(anyhow!(
                "could not place arbos-kernel at {} on {}: {}",
                dest,
                target.name,
                swap.problem()
            ));
        }
    }
    // Another machine type. The channel's feed first, because it is the
    // only place a dev build's kernel exists at all, then the tagged
    // release, then a source build where the machine allows one.
    match ssh_put_kernel_from_feed(target, remote_arch, dest, step) {
        Ok(true) => return Ok(()),
        // Nothing in the feed for this machine and this build. Not a
        // failure: a stable app finds its kernel in the release below.
        Ok(false) => {}
        Err(e) => eprintln!(
            "remote {}: install: the update feed could not place a kernel ({e:#}); trying the release",
            target.name
        ),
    }

    // The release cut for it, from GitHub, checked against its .sha256 and
    // moved into place as <bin>.new → <bin>; then the source build when the
    // machine allows it. The script says which.
    {
        let mine = arbos_core::remote_kernel::KernelVersion::parse(&local_kernel_version());
        let version = mine.as_ref().map(|m| m.short()).unwrap_or_default();
        let sha = mine.as_ref().map(|m| m.sha.clone()).unwrap_or_default();
        if !version.is_empty() {
            let script = arbos_core::remote_kernel::install_script(
                dest,
                &version,
                if sha.is_empty() { "main" } else { &sha },
                remote_arch,
                target.build,
            );
            if target.build
                && arbos_core::remote_kernel::release_asset(remote_arch, &version).is_none()
            {
                step(arbos_core::remote_kernel::Progress::Building);
            }
            let out = ssh_run(host, &script)?;
            let steps = arbos_core::remote_kernel::steps_in(&out.stdout);
            for line in &steps {
                eprintln!("remote {}: install: {line}", target.name);
            }
            if out.status == 0 {
                return Ok(());
            }
            if !target.build {
                return Err(anyhow!(
                    "arbos-kernel {version} for {there} could not be placed on {name} at {bin}: {}",
                    steps.last().cloned().unwrap_or_else(|| out.problem()),
                    there = remote_arch,
                    name = target.name,
                    bin = dest,
                ));
            }
            // Fall through: the machine allows a build; the old tarred
            // source route below is the last resort.
            eprintln!(
                "remote {}: release and cargo install did not land ({}); building from this window's source",
                target.name,
                steps.last().cloned().unwrap_or_default()
            );
        }
    }
    if replacing && !target.build {
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
cp target/release/arbos-kernel "{bin}.new"
mv -f "{bin}.new" "{bin}"
test -x "{bin}"
"#,
        bin_dir = bin_dir,
        bin = dest
    );
    let out = ssh_run(host, &script)?;
    if out.status != 0 {
        return Err(anyhow!("build arbos-kernel on {host}: {}", out.problem()));
    }
    Ok(())
}

/// Put the kernel matching *this app's own build* on a machine of another
/// type, from the channel the app follows.
///
/// This exists because the release route cannot serve a dev build. The
/// only kernel a dev build could fetch was
/// `releases/download/v<version>/arbos-kernel-…`, and for a version whose
/// tag is still an unpublished draft that URL 404s. So an app tracking
/// `dev` could not place a kernel on any remote machine at all, for as
/// long as nobody cut a release — which for someone who only ever runs
/// `main` is not a window but a permanent state. On 2026-09-17 it left
/// Jacob's ArbosLife tab dead for three hours across eight silent
/// retries.
///
/// The kernel is the one for the app's **own** build, not the newest the
/// channel has. Two ends of a tunnel that disagree are what the version
/// check at attach exists to fix, and handing the remote something newer
/// than the app would leave that check wanting to replace it again on the
/// very next attach.
///
/// Nothing new is trusted: the same feed the app updates itself from, the
/// same Ed25519 key, the same staging path and rename. The signature is
/// checked here, where the key is, and the far end is asked for
/// `--version` before the binary takes the name — the one check this side
/// cannot make, because it cannot run what it is sending.
///
/// `Ok(false)` means the feed has nothing for this machine and this
/// build, which is the ordinary answer for a stable app and a reason to
/// try the release, not an error.
fn ssh_put_kernel_from_feed(
    target: &RemoteTarget,
    remote_arch: &str,
    dest: &str,
    step: &dyn Fn(arbos_core::remote_kernel::Progress),
) -> Result<bool> {
    use arbos_update::{Channel, Component, feed, kernel as update_kernel, net, sign};
    let Some((platform, arch)) = feed::platform_of(remote_arch) else {
        return Ok(false);
    };
    let Some(key) = sign::built_in_key() else {
        // A build from before the repository had a signing key cannot
        // check a payload, so it must not install one.
        return Ok(false);
    };
    let mine = crate::build::version();

    // The channel this app follows first, then the other one.
    //
    // A build is published to whichever channel published it, and this
    // only ever installs the kernel whose version *and* build equal the
    // app's — so asking the other channel cannot bring back a different
    // build, only the same one from where it was actually published. That
    // matters because the setting and the question are not the same
    // question: an app with updates switched off still resolves to
    // `stable` here, and a dev build would find nothing there.
    let configured = crate::update::channel_now();
    let mut channels = vec![configured];
    channels.extend([Channel::Dev, Channel::Stable].into_iter().filter(|c| *c != configured));

    let mut asked: Vec<(Channel, arbos_update::Feed)> = Vec::new();
    let mut unreachable: Option<anyhow::Error> = None;
    let mut chosen = None;
    for channel in channels {
        match net::feed(channel) {
            Ok(feed) => {
                if let Some(offered) = feed.for_build(&mine, platform, arch, Component::Kernel) {
                    chosen = Some((channel, offered));
                    break;
                }
                asked.push((channel, feed));
            }
            Err(e) => {
                unreachable = Some(e.context(format!("asking the {} channel", channel.as_str())));
            }
        }
    }

    let Some((channel, offered)) = chosen else {
        // No channel could be read at all: that is a failure worth
        // reporting rather than a quiet fall-through to a route that
        // needs the same network.
        if asked.is_empty()
            && let Some(e) = unreachable
        {
            return Err(e);
        }
        // Say which of the two reasons it is, because they need different
        // things done. A channel that carries kernels but not *this* build
        // means the app has fallen off the end of that channel's retention
        // and needs to update itself; a channel with no kernel for this
        // machine at all is the stable feed's ordinary answer, and the
        // release route below is the right one.
        for (channel, feed) in &asked {
            let carried: Vec<String> = feed
                .releases
                .iter()
                .filter(|r| r.download(platform, arch, Component::Kernel).is_some())
                .map(|r| format!("{}+{}", r.version, r.build))
                .collect();
            if !carried.is_empty() {
                eprintln!(
                    "remote {}: install: this app is {} and the {} channel carries kernels for {} \
                     — it cannot place a matching one until it updates itself",
                    target.name,
                    mine.human(),
                    channel.as_str(),
                    carried.join(", ")
                );
            }
        }
        return Ok(false);
    };

    step(arbos_core::remote_kernel::Progress::Installing {
        version: offered.version.human(),
        // #462 installs from the feed: the payload's size is the feed's
        // word (#467's line says what is crossing the wire).
        bytes: offered.download.size,
    });
    eprintln!(
        "remote {}: install: {} {} for {remote_arch} from the {} channel",
        target.name,
        offered.version.human(),
        offered.commit,
        channel.as_str()
    );
    let bytes = net::bytes(&offered.download.url)
        .with_context(|| format!("fetching {}", offered.download.url))?;
    let scratch = std::env::temp_dir().join("arbos-remote-kernel");
    let _ = std::fs::remove_dir_all(&scratch);
    let placed = (|| -> Result<bool> {
        let binary = update_kernel::payload_for_another_machine(&bytes, &offered, &key, &scratch)?;
        let host = target.ssh.as_str();
        // Beside the target and then renamed over it, so nothing ever
        // reads a half-written kernel at the path it is served from.
        let tmp = format!("{dest}.new");
        ssh_put(host, &binary, &tmp)?;
        // The check this side could not make: it runs there, and it is
        // the build the feed said. A wrong architecture and a truncated
        // download both fail here rather than at the next attach.
        let says = ssh_run(host, &format!(r#"chmod +x "{tmp}" && "{tmp}" --version"#))?;
        let line = says.stdout.lines().next().unwrap_or_default().trim().to_string();
        if says.status != 0 || !line.contains(&offered.commit) {
            let _ = ssh_run(host, &format!(r#"rm -f "{tmp}""#));
            return Err(anyhow!(
                "the kernel for {remote_arch} would not run on {}: expected {} and it said {}",
                target.name,
                offered.commit,
                match line.is_empty() {
                    true => says.problem(),
                    false => line,
                }
            ));
        }
        let moved = ssh_run(host, &format!(r#"mv -f "{tmp}" "{dest}""#))?;
        if moved.status != 0 {
            let _ = ssh_run(host, &format!(r#"rm -f "{tmp}""#));
            return Err(anyhow!(
                "could not put the kernel at {dest} on {}: {}",
                target.name,
                moved.problem()
            ));
        }
        eprintln!("remote {}: install: installed from the feed", target.name);
        Ok(true)
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    placed
}

/// Where a new binary waits on the remote until it is swapped in. Under
/// the cache rather than beside the installation, so a half-finished
/// download is never one `mv` away from being the kernel.
const REMOTE_INCOMING: &str = "$HOME/.cache/arbos/arbos-kernel.incoming";

/// How long a kernel gets to end its turn after TERM, and how long the
/// pass then watches for a supervisor to put a replacement back.
///
/// The stop window is the existing `stop_script`'s 20 s. The watch window
/// is 10 s, which is the horizon the features agent asked for: long
/// enough for a `while true; do …; sleep 2; done` loop to come round,
/// short enough that a place with an hourly timer is not held open.
const REMOTE_STOP_SECS: u32 = 20;
const REMOTE_WATCH_SECS: u32 = 10;

/// Replace the kernel on a machine that already has one, and bring every
/// process that was running it onto the new build.
///
/// The new binary is staged first and swapped second, and nothing is
/// stopped until the swap has landed — see
/// [`arbos_core::remote_kernel::bootstrap_script`] for why that ordering
/// is not interchangeable with the obvious one.
///
/// This replaces the older "stop the kernel for this place, then install"
/// path. That path had two faults this one does not: it stopped before it
/// swapped, and it only ever knew about the kernel for the place being
/// opened, so the other processes sharing that binary stayed on a dead
/// image. On 2026-09-17 that left three processes on two machines running
/// builds nobody could see, one of them for five and a half hours.
fn ssh_bootstrap_kernel(
    target: &RemoteTarget,
    remote_arch: &str,
    path: &Path,
    step: &dyn Fn(arbos_core::remote_kernel::Progress),
) -> Result<()> {
    use arbos_core::remote_kernel::{Outcome, bootstrap_report, bootstrap_script};
    let host = target.ssh.as_str();
    ssh_install_kernel(target, remote_arch, REMOTE_INCOMING, true, step)?;

    step(arbos_core::remote_kernel::Progress::Stopping);
    // Real paths, not `$HOME/…`: the pass matches against
    // `/proc/<pid>/exe`, which is always absolute.
    let bin = remote_home_path(host, &target.bin)?;
    let incoming = remote_home_path(host, REMOTE_INCOMING)?;
    let script = bootstrap_script(&bin, &incoming, REMOTE_STOP_SECS, REMOTE_WATCH_SECS);
    // The pass writes its report to a file and we read it back, rather
    // than letting it stream through this session. Anything it relaunches
    // that inherited our stdout would hold this connection open for as
    // long as the kernel runs, turning a finished update into a hang.
    let out = ssh_run(
        host,
        &format!(
            r#"out="$HOME/.cache/arbos/bootstrap.out"; mkdir -p "$HOME/.cache/arbos"
{{ {script}
}} > "$out" 2>&1 </dev/null
rc=$?
cat "$out"
exit $rc"#
        ),
    )?;
    let report = bootstrap_report(&out.stdout);
    for line in &report.steps {
        eprintln!("remote {}: bootstrap: {line}", target.name);
    }
    if out.status != 0 || !report.errors.is_empty() {
        return Err(anyhow!(
            "could not bring {} on {} onto this build: {}",
            target.bin,
            target.name,
            report
                .errors
                .first()
                .cloned()
                .unwrap_or_else(|| out.problem())
        ));
    }
    step(arbos_core::remote_kernel::Progress::Restarting {
        running: report.ended.len(),
    });

    // A refusal is reported as a refusal. The place being opened is the
    // one the person is waiting on, so that one is an error; another
    // place left on its old kernel is said out loud and does not stop
    // this window from connecting, because refusing to open a project
    // over somebody else's stuck kernel helps nobody.
    let here = path.to_string_lossy();
    for stuck in report.unhappy() {
        let what = match stuck.outcome {
            Outcome::RefusedStillRunning => "would not stop, so it was left as it was",
            Outcome::Failed => "stopped but would not start again",
            Outcome::Supervised | Outcome::Relaunched | Outcome::Ended => continue,
        };
        let place = stuck.place.as_deref().unwrap_or("a kernel with no place");
        if place == here {
            return Err(anyhow!(
                "the kernel serving {place} on {} {what} (pid {})",
                target.name,
                stuck.pid
            ));
        }
        eprintln!(
            "remote {}: bootstrap: {place} {what} (pid {}); it is still on the old build",
            target.name, stuck.pid
        );
    }
    Ok(())
}

/// Stop the kernel serving `path` on the host so a newer binary can take
/// its place: TERM to that one process (its jobs die with it through
/// their leash), then wait for it to leave. An error names the pid when
/// it would not.
#[allow(dead_code)]
fn ssh_stop_kernel(target: &RemoteTarget, path: &Path) -> Result<()> {
    let host = target.ssh.as_str();
    let dir = shell_path(&path.to_string_lossy());
    let script = arbos_core::remote_kernel::stop_script(&dir);
    let out = ssh_run(host, &script)?;
    for line in arbos_core::remote_kernel::steps_in(&out.stdout) {
        eprintln!("remote {}: stop: {line}", target.name);
    }
    if out.status != 0 {
        return Err(anyhow!(
            "the kernel on {} did not stop for the update: {}",
            target.name,
            out.problem()
        ));
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

/// A remote path with a leading `$HOME` or `~` turned into the directory
/// it names, by asking the host.
///
/// Two callers need this for different reasons. `scp` over SFTP takes the
/// path as it is, with no shell there to expand it. And the bootstrap
/// pass compares paths against `/proc/<pid>/exe`, which is always a real
/// absolute path — a literal `$HOME/…` would match nothing, silently, and
/// the pass would report that it found no processes rather than that it
/// could not look.
fn remote_home_path(host: &str, remote: &str) -> Result<String> {
    if !remote.starts_with("$HOME") && !remote.starts_with('~') {
        return Ok(remote.to_string());
    }
    let home = ssh_run(host, r#"printf %s "$HOME""#)?;
    if home.status != 0 || home.stdout.trim().is_empty() {
        return Err(anyhow!(
            "could not read $HOME on {host}: {}",
            home.problem()
        ));
    }
    let rest = remote.trim_start_matches("$HOME").trim_start_matches('~');
    Ok(format!("{}{}", home.stdout.trim(), rest))
}

fn ssh_put(host: &str, local: &Path, remote: &str) -> Result<()> {
    let remote = remote_home_path(host, remote)?;
    let dest = format!("{host}:{remote}");
    // Its own connection, not the probe's mux: with ControlPersist=no the
    // probe's master is closing as scp starts, and scp through that socket
    // died with "Connection closed" every time on arboslife (cycle 11).
    let mut last = String::new();
    for attempt in 0..2 {
        let out = Command::new("scp")
            .args(ssh_base())
            .arg("-q")
            .arg(local)
            .arg(&dest)
            .stdin(Stdio::null())
            .output()
            .context("scp")?;
        if out.status.success() {
            return Ok(());
        }
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        last = if err.is_empty() {
            out.status.to_string()
        } else {
            err
        };
        if attempt == 0 {
            thread::sleep(Duration::from_millis(500));
        }
    }
    Err(anyhow!("scp {} to {host} failed: {last}", local.display()))
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
    // The launch log lives under ~/.cache/arbos, which the install makes;
    // ~/.arbos is the home place and may be anything (on templar a symlink
    // to a folder that is gone — the redirect failed and the kernel never
    // started, cycle 11). A log dir that cannot be made is a failure here,
    // not a 60 s wait for a file that never comes.
    let script = format!(
        r#"umask 077 && mkdir -p "$HOME/.cache/arbos" || {{ echo "cannot make $HOME/.cache/arbos" >&2; exit 1; }}
log="$HOME/.cache/arbos/web.log"
if command -v setsid >/dev/null 2>&1; then
  setsid nohup sh -c '{inner}' >>"$log" 2>&1 </dev/null &
else
  nohup sh -c '{inner}' >>"$log" 2>&1 </dev/null &
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
            &format!(
                "cat {dir}/.arbos/runtime/kernel.json 2>/dev/null || cat {dir}/.arbos/kernel.json 2>/dev/null"
            ),
        )?;
        if out.status == 0
            && let Ok(info) = serde_json::from_str::<WebInfo>(&out.stdout)
        {
            return Ok(info);
        }
        thread::sleep(Duration::from_millis(500));
    }
    Err(anyhow!(
        "arbos did not start within 60 s (see ~/.cache/arbos/web.log on {host})"
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
    // One level, directories only (`*/` skips hidden names), so files never
    // cross SSH; beside each name, the `kind` its `.arbos/project.toml`
    // declares, so a service or a worktree can be kept off the list on the
    // folder's own word. `head` caps the payload.
    let script = format!(
        concat!(
            "cd {} && for d in */; do d=${{d%/}}; ",
            "k=$(grep -m1 '^kind' \"$d/.arbos/project.toml\" 2>/dev/null); ",
            "printf '%s\\t%s\\n' \"$d\" \"$k\"; done 2>/dev/null | head -n {}"
        ),
        shell_path(path),
        REMOTE_DIR_CAP
    );
    let out = ssh_run_with(host, &script, &ssh_listing())?;
    if out.status != 0 {
        return Err(anyhow!(out.problem()));
    }
    let mut names: Vec<String> = out
        .stdout
        .lines()
        .filter_map(|line| {
            let (name, kind) = line.split_once('\t').unwrap_or((line, ""));
            let name = name.trim().trim_end_matches('/');
            if name.is_empty() || name == "." || name == ".." || name.starts_with('.') || name == "*" {
                return None;
            }
            if crate::model::place::hidden_kind(crate::model::place::kind_in(kind).as_deref()) {
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

/// The brief inside a spawn wake: the kernel prefixes "You were spawned by
/// agent root for this mission:" and a blank line; the card shows the
/// mission as the parent wrote it.
fn brief_of(text: &str) -> String {
    let t = text.trim();
    match t.split_once("for this mission:") {
        Some((_, rest)) => rest.trim().to_string(),
        None => t.to_string(),
    }
}

/// The prompt a spawned child was given: the wake's `brief` (the mission as
/// the parent wrote it, on wakes since #221) when present, else the mission
/// cut out of the kernel's framing in `text` (older transcripts).
fn wake_brief(text: &str, brief: Option<&str>) -> String {
    match brief.map(str::trim).filter(|b| !b.is_empty()) {
        Some(brief) => brief.to_string(),
        None => brief_of(text),
    }
}

/// The brief a worker was spawned with: the text of the first plan wake in
/// its transcript. `None` for a remote place, an id that is not a folder,
/// or a transcript that does not start with one.
/// `readonly:` and `kind:` off a local agent's `agent.md`, live or already
/// archived — a worker can finish before the listing next runs, and its
/// line still has to say it could not write.
pub fn agent_flags(place: &Place, id: &str) -> Option<(bool, Option<String>)> {
    if place.host.is_some() || !safe_session_id(id) {
        return None;
    }
    let store = place.path.join(".arbos");
    let md = [
        store.join("agents").join(id).join("agent.md"),
        store
            .join("archive")
            .join("agents")
            .join(id)
            .join("agent.md"),
    ]
    .into_iter()
    .find_map(|path| std::fs::read_to_string(path).ok())?;
    let front = agent_front(&md);
    Some((front.readonly, front.kind))
}

/// The kernel's own word on who an agent belongs to, from its `agent.md`
/// (live or archived): `None` when the kernel has no record of the agent
/// at all, `Some(None)` for a parentless chat, `Some(Some(parent))` for a
/// worker. The window's session file is a cache of this, never the source
/// (F-137: a parentless `chat-…` the panel had adopted under root drew as
/// "Delegate 1" for two days). Remote places have no file to read and
/// answer `None`; their roster is the record there.
pub fn agent_parent(place: &Place, id: &str) -> Option<Option<String>> {
    if place.host.is_some() || !safe_session_id(id) {
        return None;
    }
    let store = place.path.join(".arbos");
    let md = [
        store.join("agents").join(id).join("agent.md"),
        store
            .join("archive")
            .join("agents")
            .join(id)
            .join("agent.md"),
    ]
    .into_iter()
    .find_map(|path| std::fs::read_to_string(path).ok())?;
    Some(agent_front(&md).parent)
}

pub fn agent_brief(place: &Place, id: &str) -> Option<String> {
    if place.host.is_some() || !safe_session_id(id) {
        return None;
    }
    let path = place
        .path
        .join(".arbos")
        .join("agents")
        .join(id)
        .join("transcript.jsonl");
    let file = std::fs::File::open(path).ok()?;
    let mut first = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(file), &mut first).ok()?;
    let ev: arbos_core::Event = serde_json::from_str(first.trim()).ok()?;
    match ev.kind {
        arbos_core::EventKind::Wake {
            wake,
            text: Some(text),
            brief,
        } if wake == "plan" && !text.trim().is_empty() => Some(wake_brief(&text, brief.as_deref())),
        _ => None,
    }
}
