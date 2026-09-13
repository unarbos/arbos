//! A child on another machine, reached over SSH.
//!
//! `spawn host=<name>` syncs the project to that machine's dedicated
//! directory, puts an `arbos-kernel` there when it lacks one, starts it,
//! opens an `ssh -N -L` tunnel to its attach port, and sends the brief to
//! its root agent. Locally the child is an agent folder with `remote:
//! <machine>:<path>` in `agent.md`; its transcript mirrors the remote one,
//! and the remote's replies reach the parent as messages from the child.
//!
//! This is the design's "clients are views onto the kernel that owns the
//! folder" with SSH as the carrier; the hub and WebSocket route replaces
//! the tunnel later without changing what the parent or the child sees.

use anyhow::{Context, Result, bail};
use arbos_core::{
    Agent, AgentId, Event, EventKind, Layout, Machine, Machines, Node, Place, append_event,
    append_events, node::DEFAULT_HOPS, validate_id, wire::Frame,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use crate::hooks::KernelHooks;

/// How long to wait for the remote kernel to write its port, and for the
/// tunnel to accept.
const REMOTE_READY: Duration = Duration::from_secs(60);
const TUNNEL_READY: Duration = Duration::from_secs(20);
/// After the remote says idle, the time its transcript tail has to settle.
const SETTLE: Duration = Duration::from_millis(1500);

/// One remote child, as remembered across kernel restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub agent: String,
    pub parent: String,
    pub machine: String,
    pub path: String,
    /// Lines of the remote root's transcript already copied here.
    #[serde(default)]
    pub mirrored: usize,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RecordsFile {
    remotes: Vec<Record>,
}

impl RecordsFile {
    fn path(place: &Place) -> PathBuf {
        place.arbos().join("remotes.json")
    }
    fn load(place: &Place) -> Self {
        std::fs::read_to_string(Self::path(place))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }
    fn save(&self, place: &Place) -> Result<()> {
        let path = Self::path(place);
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}

/// A live link to one remote child.
pub struct Link {
    pub record: Record,
    machine: Machine,
    /// Frames for the remote kernel; the relay task writes them.
    to_remote: mpsc::UnboundedSender<Frame>,
    tunnel: Mutex<Option<Child>>,
}

#[derive(Default)]
pub struct RemoteHub {
    links: Mutex<HashMap<String, Arc<Link>>>,
}

impl RemoteHub {
    pub fn link(&self, agent: &str) -> Option<Arc<Link>> {
        self.links.lock().unwrap().get(agent).cloned()
    }

    /// Words for a remote child: mirrored on its local transcript for the
    /// window, sent to the remote root as a user line (a steer when asked).
    pub fn forward(
        &self,
        hooks: &KernelHooks,
        agent: &str,
        from: &str,
        text: &str,
        steer: bool,
    ) -> Result<()> {
        let link = self
            .link(agent)
            .ok_or_else(|| anyhow::anyhow!("{agent} runs on another machine and its link is down; a kernel restart re-attaches it"))?;
        append_event(
            &hooks.layout(agent).transcript(),
            &Event::new(EventKind::Say {
                from: from.to_string(),
                text: text.to_string(),
            }),
        )?;
        link.to_remote
            .send(Frame::User {
                agent: "root".into(),
                text: format!("[{from}] {text}"),
                steer,
                attachments: vec![],
            })
            .map_err(|_| anyhow::anyhow!("the link to {} closed", link.record.machine))?;
        Ok(())
    }

    /// Re-attach every remembered remote child at kernel start.
    pub fn restore(hooks: &Arc<KernelHooks>) {
        let file = RecordsFile::load(&hooks.place);
        if file.remotes.is_empty() {
            return;
        }
        let machines = match Machines::load() {
            Ok(m) => m,
            Err(e) => {
                eprintln!("remote: {e:#}");
                return;
            }
        };
        for record in file.remotes {
            let Some(machine) = machines.get(&record.machine).cloned() else {
                eprintln!(
                    "remote: {} is not in machines.toml; {} stays detached",
                    record.machine, record.agent
                );
                continue;
            };
            let hooks = Arc::clone(hooks);
            tokio::spawn(async move {
                let h = Arc::clone(&hooks);
                let r = record.clone();
                let m = machine.clone();
                let ready = tokio::task::spawn_blocking(move || {
                    let port = start_remote_kernel(&m, &r.path)?;
                    open_tunnel(&m, port)
                })
                .await;
                match ready {
                    Ok(Ok((child, local_port))) => {
                        if let Err(e) =
                            attach(hooks, machine, record.clone(), child, local_port, None).await
                        {
                            eprintln!("remote: re-attach {}: {e:#}", record.agent);
                        }
                    }
                    Ok(Err(e)) => {
                        eprintln!("remote: re-attach {}: {e:#}", record.agent);
                        let _ = append_event(
                            &h.layout(&record.agent).transcript(),
                            &Event::new(EventKind::Notice {
                                text: format!(
                                    "could not re-attach to {} at {}: {e:#}",
                                    record.machine, record.path
                                ),
                                failed: true,
                            }),
                        );
                    }
                    Err(e) => eprintln!("remote: re-attach task: {e}"),
                }
            });
        }
    }
}

/// `spawn host=<name>`: the whole road from a brief to a child working on
/// another machine. Blocking work (ssh, rsync, scp) runs on the caller's
/// blocking thread; the relay is an async task.
pub async fn spawn_remote(
    hooks: Arc<KernelHooks>,
    parent: Agent,
    brief: String,
    host: String,
) -> Result<(AgentId, String)> {
    let machines = Machines::load()?;
    let machine = machines.get(&host).cloned().ok_or_else(|| {
        anyhow::anyhow!(
            "no machine named {host:?} in {}. Known: {}",
            Machines::path().display(),
            if machines.machine.is_empty() {
                "(none; add a [[machine]] with name, ssh, dir)".to_string()
            } else {
                machines
                    .machine
                    .iter()
                    .map(|m| m.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        )
    })?;
    let place = hooks.place.clone();
    let project = place
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("project")
        .to_string();
    // The local stand-in first, so the window shows the child while the
    // sync runs. Same id rules as a local spawn. Each child gets a place
    // of its own on the machine — its own copy, kernel, and root — so two
    // children never share a transcript.
    let id = hooks.remote_child_id(&brief)?;
    validate_id(&id)?;
    let remote_path = machine.place_for_child(&project, &id);
    let mut child = Agent::root(&id);
    child.name = brief.chars().take(48).collect();
    child.parent = Some(parent.id.clone());
    child.model = parent.model.clone();
    child.allowlist = parent.allowlist.clone();
    child.remote = Some(format!("{}:{}", machine.name, remote_path));
    child.save(&place.agent_dir(&id))?;
    std::fs::create_dir_all(Layout::new(&place, &id).jobs())?;
    append_event(
        &Layout::new(&place, &id).transcript(),
        &Event::new(EventKind::Notice {
            text: format!(
                "Runs on {} ({}) at {}. Syncing the project and starting a kernel there…",
                machine.name, machine.ssh, remote_path
            ),
            failed: false,
        }),
    )?;
    hooks.broadcast_tree();

    let m = machine.clone();
    let rp = remote_path.clone();
    let local_place = place.path.clone();
    let prepared = tokio::task::spawn_blocking(move || -> Result<(u16, Child, u16, String)> {
        let notes = prepare_remote(&m, &local_place, &rp)?;
        let port = start_remote_kernel(&m, &rp)?;
        let (tunnel, local_port) = open_tunnel(&m, port)?;
        Ok((port, tunnel, local_port, notes))
    })
    .await
    .map_err(|e| anyhow::anyhow!("remote task: {e}"))?;
    let (remote_port, tunnel, local_port, notes) = match prepared {
        Ok(v) => v,
        Err(e) => {
            // No child came of it: the stand-in goes, as a failed local
            // spawn leaves no folder either.
            let _ = std::fs::remove_dir_all(place.agent_dir(&id));
            hooks.broadcast_tree();
            return Err(e);
        }
    };
    // Whatever the remote root did before this brief is not this child's
    // story: mirror from here on.
    let m2 = machine.clone();
    let rp2 = remote_path.clone();
    let already = tokio::task::spawn_blocking(move || remote_transcript_len(&m2, &rp2))
        .await
        .map_err(|e| anyhow::anyhow!("remote task: {e}"))?
        .unwrap_or(0);
    let record = Record {
        agent: id.clone(),
        parent: parent.id.to_string(),
        machine: machine.name.clone(),
        path: remote_path.clone(),
        mirrored: already,
    };
    let mut file = RecordsFile::load(&place);
    file.remotes.retain(|r| r.agent != id);
    file.remotes.push(record.clone());
    file.save(&place)?;
    append_event(
        &Layout::new(&place, &id).transcript(),
        &Event::new(EventKind::Notice {
            text: format!(
                "{notes}Kernel on {} listens on 127.0.0.1:{remote_port} there; tunnelled to 127.0.0.1:{local_port} here.",
                machine.name
            ),
            failed: false,
        }),
    )?;
    let first = format!(
        "You were spawned by agent {} on another machine for this mission:\n\n{brief}\n\nYou work in {remote_path} on {}, a copy of the project synced from the parent's machine. Do it now; report results in your final reply — it is delivered to your parent as a message from you. If you change files, commit them on a branch and name it in the report.",
        parent.id, machine.name
    );
    attach(
        Arc::clone(&hooks),
        machine.clone(),
        record,
        tunnel,
        local_port,
        Some(first),
    )
    .await?;
    Ok((
        AgentId::new(id),
        format!(
            "on {} at {remote_path} (its reports arrive here as messages from it; say to={} reaches it)",
            machine.name, child.id
        ),
    ))
}

/// Connect through the tunnel, send the brief (when given), and run the
/// relay for the life of the link.
async fn attach(
    hooks: Arc<KernelHooks>,
    machine: Machine,
    record: Record,
    tunnel: Child,
    local_port: u16,
    first: Option<String>,
) -> Result<()> {
    let stream = TcpStream::connect(("127.0.0.1", local_port))
        .await
        .with_context(|| format!("connect the tunnel to {}", machine.name))?;
    let (r, mut w) = stream.into_split();
    let mut lines = BufReader::new(r).lines();
    // The kernel greets every client with a snapshot. No greeting means
    // the tunnel is up but nothing answers behind it (a stale port).
    match tokio::time::timeout(TUNNEL_READY, lines.next_line()).await {
        Ok(Ok(Some(_))) => {}
        Ok(Ok(None)) | Ok(Err(_)) => bail!(
            "the tunnel to {} opened but the kernel there closed the connection; see {}/.arbos/kernel.log on it",
            machine.name,
            record.path
        ),
        Err(_) => bail!(
            "no answer from the kernel on {} within {TUNNEL_READY:?}",
            machine.name
        ),
    }
    let (tx, mut rx) = mpsc::unbounded_channel::<Frame>();
    let link = Arc::new(Link {
        record: record.clone(),
        machine: machine.clone(),
        to_remote: tx,
        tunnel: Mutex::new(Some(tunnel)),
    });
    hooks
        .remotes
        .links
        .lock()
        .unwrap()
        .insert(record.agent.clone(), Arc::clone(&link));
    let mirrored = record.mirrored;
    if let Some(text) = first {
        link.to_remote
            .send(Frame::User {
                agent: "root".into(),
                text,
                steer: false,
                attachments: vec![],
            })
            .ok();
    }
    tokio::spawn(async move {
        let mut running = false;
        let mut idle_at: Option<Instant> = None;
        let mut mirrored = mirrored;
        loop {
            tokio::select! {
                out = rx.recv() => {
                    let Some(frame) = out else { break };
                    if let Ok(s) = serde_json::to_string(&frame) {
                        if w.write_all(format!("{s}\n").as_bytes()).await.is_err() {
                            break;
                        }
                    }
                }
                line = lines.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            let Ok(frame) = serde_json::from_str::<Frame>(&line) else { continue };
                            if let Frame::Turn { agent, state, .. } = frame
                                && agent == "root"
                            {
                                if state == "running" {
                                    running = true;
                                    idle_at = None;
                                } else if state == "idle" && running {
                                    idle_at = Some(Instant::now());
                                }
                            }
                        }
                        _ => break,
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(300)) => {}
            }
            if let Some(at) = idle_at
                && at.elapsed() >= SETTLE
            {
                idle_at = None;
                running = false;
                let m = link.machine.clone();
                let path = link.record.path.clone();
                let from = mirrored;
                let fetched =
                    tokio::task::spawn_blocking(move || remote_transcript_tail(&m, &path, from))
                        .await;
                match fetched {
                    Ok(Ok(events)) if !events.is_empty() => {
                        mirrored += events.len();
                        let _ =
                            append_events(&hooks.layout(&link.record.agent).transcript(), &events);
                        // Remember where the mirror stands for a restart.
                        let mut file = RecordsFile::load(&hooks.place);
                        for r in &mut file.remotes {
                            if r.agent == link.record.agent {
                                r.mirrored = mirrored;
                            }
                        }
                        let _ = file.save(&hooks.place);
                        let reply = events
                            .iter()
                            .rev()
                            .find_map(|e| match &e.kind {
                                EventKind::Assistant { text, .. } if !text.trim().is_empty() => {
                                    Some(text.clone())
                                }
                                _ => None,
                            })
                            .or_else(|| {
                                events.iter().rev().find_map(|e| match &e.kind {
                                    EventKind::Notice { text, failed: true } => {
                                        Some(format!("(failed on {}) {text}", link.machine.name))
                                    }
                                    _ => None,
                                })
                            });
                        if let Some(text) = reply {
                            if let Err(e) = deliver(&hooks, &link.record, &text) {
                                eprintln!("remote: deliver from {}: {e:#}", link.record.agent);
                            }
                        }
                        hooks.broadcast_tree();
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => {
                        eprintln!("remote: read transcript on {}: {e:#}", link.machine.name)
                    }
                    Err(e) => eprintln!("remote: transcript task: {e}"),
                }
            }
        }
        // The socket or the tunnel went away.
        hooks
            .remotes
            .links
            .lock()
            .unwrap()
            .remove(&link.record.agent);
        if let Some(mut t) = link.tunnel.lock().unwrap().take() {
            let _ = t.kill();
        }
        let _ = append_event(
            &hooks.layout(&link.record.agent).transcript(),
            &Event::new(EventKind::Notice {
                text: format!(
                    "the link to {} closed; a kernel restart re-attaches it",
                    link.machine.name
                ),
                failed: true,
            }),
        );
        let _ = deliver(
            &hooks,
            &link.record,
            &format!(
                "(link to {} lost; the remote kernel may still be working — restart this kernel to re-attach)",
                link.machine.name
            ),
        );
    });
    Ok(())
}

/// The remote reply as a message from the child on the parent's
/// transcript, and a turn for the parent — `say mode=request` from the child.
fn deliver(hooks: &KernelHooks, record: &Record, text: &str) -> Result<()> {
    append_event(
        &hooks.layout(&record.parent).transcript(),
        &Event::new(EventKind::Say {
            from: record.agent.clone(),
            text: text.to_string(),
        }),
    )?;
    let mut n = Node::inbox(text, format!("agent:{}", record.agent));
    n.hops = DEFAULT_HOPS;
    hooks.inbox(&record.parent, n)?;
    Ok(())
}

// ── ssh plumbing (blocking) ─────────────────────────────────────────────

/// The private key as a file path, reading an `op://` reference once into
/// `~/.config/arbos/keys/<machine>` (mode 600).
fn key_file(machine: &Machine) -> Result<Option<PathBuf>> {
    let Some(key) = machine
        .key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
    else {
        return Ok(None);
    };
    if !key.starts_with("op://") {
        return Ok(Some(PathBuf::from(shellexpand_home(key))));
    }
    let dir = arbos_core::host_dir().join("keys");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(&machine.name);
    if !path.exists() {
        let out = Command::new("op")
            .args(["read", key])
            .stdin(Stdio::null())
            .output()
            .context("run op (is the 1Password CLI installed and OP_SERVICE_ACCOUNT_TOKEN set?)")?;
        if !out.status.success() {
            bail!(
                "op read for {}'s key: {}",
                machine.name,
                String::from_utf8_lossy(&out.stderr)
                    .lines()
                    .last()
                    .unwrap_or("failed")
            );
        }
        std::fs::write(&path, &out.stdout)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    Ok(Some(path))
}

fn shellexpand_home(p: &str) -> String {
    match (p.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => format!("{}/{rest}", home.to_string_lossy()),
        _ => p.to_string(),
    }
}

/// The options every ssh/scp/rsync call shares.
fn ssh_opts(machine: &Machine) -> Result<Vec<String>> {
    let mut o = vec![
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        "-o".into(),
        "ConnectTimeout=15".into(),
    ];
    if let Some(k) = key_file(machine)? {
        o.push("-i".into());
        o.push(k.display().to_string());
    }
    Ok(o)
}

fn ssh_run(machine: &Machine, script: &str) -> Result<String> {
    let mut cmd = Command::new("ssh");
    cmd.args(ssh_opts(machine)?);
    if let Some(p) = machine.port {
        cmd.arg("-p").arg(p.to_string());
    }
    cmd.arg(machine.target()).arg(script).stdin(Stdio::null());
    let out = cmd.output().context("run ssh")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!(
            "ssh {}: {}",
            machine.target(),
            err.lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("failed")
                .trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Sync the project, make sure a kernel binary and a model config are
/// there. Returns notes for the child's transcript.
fn prepare_remote(machine: &Machine, local_place: &Path, remote_path: &str) -> Result<String> {
    let mut notes = String::new();
    let dir = machine.dir.trim_end_matches('/');
    ssh_run(machine, &format!("mkdir -p {} {dir}/bin", sq(remote_path)))?;

    // The working tree, not the state: `.arbos/` belongs to each kernel.
    // rsync when this machine has it; a tar stream over ssh otherwise.
    let excludes = [".arbos", "target", "node_modules"];
    let mut ssh_cmd = vec!["ssh".to_string()];
    ssh_cmd.extend(ssh_opts(machine)?);
    if let Some(p) = machine.port {
        ssh_cmd.push("-p".into());
        ssh_cmd.push(p.to_string());
    }
    let synced_with = if Command::new("rsync")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
    {
        let src = format!("{}/", local_place.display());
        let dst = format!("{}:{}/", machine.target(), remote_path);
        let mut args: Vec<String> = vec!["-az".into(), "--delete".into()];
        for e in excludes {
            args.push("--exclude".into());
            args.push(format!("{e}/"));
        }
        args.push("-e".into());
        args.push(ssh_cmd.join(" "));
        args.push(src);
        args.push(dst);
        let out = Command::new("rsync")
            .args(&args)
            .stdin(Stdio::null())
            .output()
            .context("run rsync")?;
        if !out.status.success() {
            bail!(
                "rsync to {}: {}",
                machine.name,
                String::from_utf8_lossy(&out.stderr)
                    .lines()
                    .last()
                    .unwrap_or("failed")
                    .trim()
            );
        }
        "rsync"
    } else {
        let mut tar = Command::new("tar");
        tar.arg("-czf").arg("-");
        for e in excludes {
            tar.arg("--exclude").arg(format!("./{e}"));
        }
        tar.arg("-C").arg(local_place).arg(".");
        tar.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut tar_child = tar.spawn().context("run tar")?;
        let mut remote = Command::new("ssh");
        remote.args(ssh_opts(machine)?);
        if let Some(p) = machine.port {
            remote.arg("-p").arg(p.to_string());
        }
        remote
            .arg(machine.target())
            .arg(format!(
                "mkdir -p {p} && tar -xzf - -C {p}",
                p = sq(remote_path)
            ))
            .stdin(tar_child.stdout.take().expect("tar stdout"))
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let out = remote.output().context("run ssh tar")?;
        let _ = tar_child.wait();
        if !out.status.success() {
            bail!(
                "tar over ssh to {}: {}",
                machine.name,
                String::from_utf8_lossy(&out.stderr)
                    .lines()
                    .last()
                    .unwrap_or("failed")
                    .trim()
            );
        }
        "tar over ssh"
    };
    notes.push_str(&format!(
        "Project synced ({synced_with}, without .arbos/, target/, node_modules/). "
    ));

    // A kernel there: theirs, or a copy of this one when the architecture
    // matches.
    let kernel = machine.kernel_path();
    let config_home = machine.config_home();
    let probe = ssh_run(
        machine,
        &format!(
            "if test -x {k}; then echo have; else echo none; fi; uname -sm; test -f {c}/arbos/config.toml && echo config || echo noconfig",
            k = sq(&kernel),
            c = sq(&config_home)
        ),
    )?;
    let mut probe_lines = probe.lines();
    let have = probe_lines.next().unwrap_or("none") == "have";
    let arch = probe_lines.next().unwrap_or("").trim().to_string();
    let has_config = probe_lines.next().unwrap_or("noconfig") == "config";
    if !have {
        let local_arch = format!(
            "{} {}",
            match std::env::consts::OS {
                "linux" => "Linux",
                "macos" => "Darwin",
                o => o,
            },
            std::env::consts::ARCH
        );
        if local_arch != arch {
            bail!(
                "{} has no arbos-kernel at {kernel} and runs {arch}, not {local_arch}, so this kernel's binary will not run there. Build one on that machine (clone the repo; cargo build --release -p arbos-kernel; copy target/release/arbos-kernel to {kernel}) or set kernel = \"<path>\" for it in machines.toml.",
                machine.name
            );
        }
        let me = std::env::current_exe().context("locate this kernel binary")?;
        scp(machine, &me, &kernel)?;
        ssh_run(machine, &format!("chmod 755 {}", sq(&kernel)))?;
        notes.push_str(&format!("Installed arbos-kernel at {kernel}. "));
    }
    if !has_config {
        // The remote kernel needs a model key of its own. The one this
        // kernel resolved goes over as a 0600 file, only when none is there.
        let host = arbos_engine::Host::load()?;
        let Some(key) = host.api_key() else {
            bail!(
                "this kernel has no model API key to give {}, and it has no {}/arbos/config.toml",
                machine.name,
                config_home
            );
        };
        let mut cfg = host.config.clone();
        cfg.api_key = Some(key);
        cfg.api_key_env = None;
        let text = toml::to_string_pretty(&cfg)?;
        let tmp =
            std::env::temp_dir().join(format!("arbos-remote-config-{}.toml", std::process::id()));
        std::fs::write(&tmp, text)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        ssh_run(
            machine,
            &format!(
                "mkdir -p {c}/arbos && chmod 700 {c} {c}/arbos",
                c = sq(&config_home)
            ),
        )?;
        let res = scp(machine, &tmp, &format!("{config_home}/arbos/config.toml"));
        let _ = std::fs::remove_file(&tmp);
        res?;
        ssh_run(
            machine,
            &format!("chmod 600 {}/arbos/config.toml", sq(&config_home)),
        )?;
        notes.push_str(&format!(
            "Wrote {config_home}/arbos/config.toml there (model and key, mode 600). "
        ));
    }
    Ok(notes)
}

fn scp(machine: &Machine, from: &Path, to: &str) -> Result<()> {
    let mut cmd = Command::new("scp");
    cmd.args(ssh_opts(machine)?);
    if let Some(p) = machine.port {
        cmd.arg("-P").arg(p.to_string());
    }
    cmd.arg("-q")
        .arg(from)
        .arg(format!("{}:{to}", machine.target()))
        .stdin(Stdio::null());
    let out = cmd.output().context("run scp")?;
    if !out.status.success() {
        bail!(
            "scp {} to {}: {}",
            from.display(),
            machine.name,
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .last()
                .unwrap_or("failed")
                .trim()
        );
    }
    Ok(())
}

/// Start `arbos-kernel serve` at `path` there unless one is alive; return
/// the port from its kernel.json.
fn start_remote_kernel(machine: &Machine, path: &str) -> Result<u16> {
    let kernel = machine.kernel_path();
    // Everything the kernel writes for itself — config, tgrep cache — stays
    // under the dedicated directory, not in the account's own dot-folders.
    let env = format!(
        "XDG_CONFIG_HOME={} XDG_CACHE_HOME={}",
        sq(&machine.config_home()),
        sq(&format!("{}/cache", machine.dir.trim_end_matches('/')))
    );
    let script = format!(
        r#"cd {p} && mkdir -p .arbos && \
if test -f .arbos/kernel.json && pid=$(sed -n 's/.*"pid": *\([0-9]*\).*/\1/p' .arbos/kernel.json) && test -n "$pid" && kill -0 "$pid" 2>/dev/null; then :; else \
  rm -f .arbos/kernel.json; \
  if command -v setsid >/dev/null 2>&1; then ( {env} setsid nohup {k} serve {p} > .arbos/kernel.log 2>&1 < /dev/null & ); \
  else ( {env} nohup {k} serve {p} > .arbos/kernel.log 2>&1 < /dev/null & ); fi; \
  for i in $(seq 1 60); do test -f .arbos/kernel.json && grep -q tcp .arbos/kernel.json && break; sleep 1; done; \
fi; cat .arbos/kernel.json"#,
        p = sq(path),
        k = sq(&kernel)
    );
    let started = Instant::now();
    loop {
        let out = ssh_run(machine, &script)?;
        if let Some(port) = out
            .split("tcp://127.0.0.1:")
            .nth(1)
            .and_then(|s| {
                s.trim_matches(|c: char| !c.is_ascii_digit())
                    .parse::<u16>()
                    .ok()
            })
            .or_else(|| {
                out.split("tcp://")
                    .nth(1)
                    .and_then(|s| s.split(':').nth(1))
                    .and_then(|s| {
                        s.trim_matches(|c: char| !c.is_ascii_digit())
                            .parse::<u16>()
                            .ok()
                    })
            })
        {
            return Ok(port);
        }
        if started.elapsed() > REMOTE_READY {
            bail!(
                "the kernel on {} did not write {}/.arbos/kernel.json; see {}/.arbos/kernel.log there",
                machine.name,
                path,
                path
            );
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// `ssh -N -L <local>:127.0.0.1:<remote>`; returns the process and the port.
fn open_tunnel(machine: &Machine, remote_port: u16) -> Result<(Child, u16)> {
    let local_port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0")?;
        l.local_addr()?.port()
    };
    let mut cmd = Command::new("ssh");
    cmd.args(ssh_opts(machine)?);
    if let Some(p) = machine.port {
        cmd.arg("-p").arg(p.to_string());
    }
    cmd.arg("-N")
        .arg("-o")
        .arg("ExitOnForwardFailure=yes")
        .arg("-o")
        .arg("ServerAliveInterval=30")
        .arg("-L")
        .arg(format!("127.0.0.1:{local_port}:127.0.0.1:{remote_port}"))
        .arg(machine.target())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = cmd.spawn().context("start the ssh tunnel")?;
    let started = Instant::now();
    loop {
        if std::net::TcpStream::connect_timeout(
            &format!("127.0.0.1:{local_port}").parse().unwrap(),
            Duration::from_millis(500),
        )
        .is_ok()
        {
            return Ok((child, local_port));
        }
        if let Ok(Some(status)) = child.try_wait() {
            bail!("the ssh tunnel to {} exited ({status})", machine.name);
        }
        if started.elapsed() > TUNNEL_READY {
            let _ = child.kill();
            bail!("the ssh tunnel to {} did not come up", machine.name);
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// How many lines the remote root's transcript has now.
fn remote_transcript_len(machine: &Machine, path: &str) -> Result<usize> {
    let out = ssh_run(
        machine,
        &format!(
            "wc -l < {}/.arbos/agents/root/transcript.jsonl 2>/dev/null || echo 0",
            sq(path)
        ),
    )?;
    Ok(out.trim().parse().unwrap_or(0))
}

/// Lines `from..` of the remote root's transcript, parsed.
fn remote_transcript_tail(machine: &Machine, path: &str, from: usize) -> Result<Vec<Event>> {
    let out = ssh_run(
        machine,
        &format!(
            "tail -n +{} {}/.arbos/agents/root/transcript.jsonl 2>/dev/null || true",
            from + 1,
            sq(path)
        ),
    )?;
    let mut events = Vec::new();
    for line in out.lines() {
        if let Ok(mut e) = serde_json::from_str::<Event>(line) {
            e.seq = 0;
            events.push(e);
        }
    }
    Ok(events)
}

/// Single-quote for a POSIX shell.
fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
