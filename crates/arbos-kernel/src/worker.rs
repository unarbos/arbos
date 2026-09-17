//! `arbos-kernel worker`: this machine, offered to the mesh.
//!
//! The worker dials the hub outbound and registers as `--machine <name>`
//! with the checkouts under `--dir`. When a remote Arbos claims it for a
//! project, the worker starts `arbos-kernel serve` for that checkout on
//! this machine, with `--hub` so the new kernel registers itself; the hub
//! then joins the claimer to it. The claimer's brief and permission mode
//! arrive over that attach. Nothing is synced: the work happens in the
//! checkout this machine already has, the way Cursor's self-hosted worker
//! runs in the repo on your laptop. With `isolate`, the kernel serves a
//! git worktree of the checkout (`.arbos/worktrees/<claim>`, branch
//! `arbos/<claim>`), so the machine's own working copy is never edited.

use anyhow::{Context, Result, bail};
use arbos_core::hub::{HubConfig, HubFrame, RegistrantKind};
use futures_util::SinkExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

use crate::hub_link;

pub const USAGE: &str = "arbos-kernel worker --dir DIR [--hub URL] [--machine NAME] [--cap NAME]... [--label WORD]...   (offer this machine to the mesh: a remote `spawn host=NAME` starts a kernel in DIR/<project>; hub URL, machine, and token from ~/.config/arbos/hub.toml or ARBOS_HUB / ARBOS_HUB_MACHINE / ARBOS_HUB_TOKEN)";

/// How long the new kernel has to write its port.
const KERNEL_READY: Duration = Duration::from_secs(60);
const PING_EVERY: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct Args {
    pub dir: PathBuf,
    pub hub: Option<String>,
    pub machine: Option<String>,
    pub capabilities: Vec<String>,
    pub labels: Vec<String>,
}

impl Args {
    pub fn parse(mut argv: impl Iterator<Item = String>) -> Result<Self> {
        let mut args = Self {
            dir: PathBuf::new(),
            hub: None,
            machine: None,
            capabilities: Vec::new(),
            labels: Vec::new(),
        };
        while let Some(a) = argv.next() {
            match a.as_str() {
                "--dir" | "-d" => {
                    args.dir = PathBuf::from(argv.next().context("--dir needs a directory")?)
                }
                "--hub" => args.hub = Some(argv.next().context("--hub needs a wss:// url")?),
                "--machine" => args.machine = Some(argv.next().context("--machine needs a name")?),
                "--cap" => args
                    .capabilities
                    .push(argv.next().context("--cap needs a name")?),
                "--label" => args
                    .labels
                    .push(argv.next().context("--label needs a word")?),
                "-h" | "--help" => {
                    println!("{USAGE}");
                    std::process::exit(0);
                }
                other => bail!("worker: unknown argument {other}\n{USAGE}"),
            }
        }
        if args.dir.as_os_str().is_empty() {
            bail!("worker: --dir is required\n{USAGE}");
        }
        Ok(args)
    }
}

/// `serve --leash` for a worktree kernel: alone and idle this long, it
/// exits; the worktree stays on disk for the branch.
const WORKTREE_LEASH: &str = "10m";

/// The checkouts this machine offers: every non-hidden folder under `dir`.
fn projects_in(dir: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = rd
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| !n.starts_with('.'))
        .collect();
    out.sort();
    out
}

pub fn run(args: Args) -> Result<i32> {
    let dir = std::fs::canonicalize(&args.dir)
        .with_context(|| format!("--dir {}: not a directory", args.dir.display()))?;
    // Flags, then the environment `serve` also reads, then hub.toml.
    let url = args
        .hub
        .clone()
        .or_else(|| std::env::var(hub_link::URL_ENV).ok());
    let machine = args
        .machine
        .clone()
        .or_else(|| std::env::var(hub_link::MACHINE_ENV).ok());
    let cfg = HubConfig::resolve(url.as_deref(), machine.as_deref())?.context(
        "worker: no hub; pass --hub wss://… (or ARBOS_HUB) or write ~/.config/arbos/hub.toml",
    )?;
    // Fail now, not at the first claim, when the token is missing.
    cfg.token()?;
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        let mut attempt = 0u32;
        loop {
            match session(&cfg, &dir, &args).await {
                Ok(()) => {
                    attempt = 0;
                    eprintln!("worker: hub closed the socket; reconnecting");
                }
                Err(e) => {
                    eprintln!("worker: {e:#}; retry in {:?}", hub_link::backoff(attempt));
                }
            }
            tokio::time::sleep(hub_link::backoff(attempt)).await;
            attempt = attempt.saturating_add(1);
        }
    })
}

async fn session(cfg: &HubConfig, dir: &Path, args: &Args) -> Result<()> {
    let token = cfg.token()?;
    let mut ws = hub_link::connect(&cfg.register_url(), &token).await?;
    let projects = projects_in(dir);
    // Each checkout's face, for the roster: a phone lists projects it has
    // never opened with the glyph and colour the desktop's tab set.
    let identities: std::collections::BTreeMap<_, _> = projects
        .iter()
        .filter_map(|p| arbos_core::project::identity_at(&dir.join(p)).map(|i| (p.clone(), i)))
        .collect();
    // And each checkout's sharing mode, so the hub can say who may reach
    // its store by address once a kernel serves it.
    let shares: std::collections::BTreeMap<_, _> = projects
        .iter()
        .filter_map(|p| {
            arbos_core::project::share_mode_set_at(&dir.join(p)).map(|m| (p.clone(), m.to_string()))
        })
        .collect();
    let kinds: std::collections::BTreeMap<_, _> = projects
        .iter()
        .filter_map(|p| {
            arbos_core::project::kind_at(&dir.join(p)).map(|k| (p.clone(), k.to_string()))
        })
        .collect();
    let id = hub_link::register(
        &mut ws,
        cfg,
        RegistrantKind::Worker,
        None,
        Some(dir.display().to_string()),
        projects.clone(),
        &args.labels,
        args.capabilities.clone(),
        identities,
        shares,
        kinds,
    )
    .await?;
    println!(
        "arbos-kernel worker: registered as {} on {} (id {id}); checkouts: {}",
        cfg.machine,
        cfg.url,
        if projects.is_empty() {
            "none".to_string()
        } else {
            projects.join(", ")
        }
    );
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    loop {
        tokio::select! {
            line = hub_link::next_text(&mut ws) => {
                let Some(line) = line else { return Ok(()) };
                let Ok(f) = serde_json::from_str::<HubFrame>(&line) else { continue };
                match f {
                    HubFrame::Claim { id, project, isolate, from } => {
                        println!("worker: claim {id} from {from}: project {project} isolate={isolate}");
                        let cfg2 = cfg.clone();
                        let dir2 = dir.to_path_buf();
                        let (project2, id2) = (project.clone(), id.clone());
                        let outcome = tokio::task::spawn_blocking(move || {
                            start_kernel(&cfg2, &dir2, &project2, &id2, isolate)
                        })
                        .await
                        .map_err(|e| anyhow::anyhow!("claim task: {e}"))?;
                        let answer = match outcome {
                            Ok((served, place)) => {
                                println!("worker: claim {id}: kernel for {served} at {}", place.display());
                                HubFrame::Claimed {
                                    id,
                                    machine: cfg.machine.clone(),
                                    project: served,
                                    place: place.display().to_string(),
                                    ok: true,
                                    detail: String::new(),
                                }
                            }
                            Err(e) => {
                                eprintln!("worker: claim {id} failed: {e:#}");
                                HubFrame::Claimed {
                                    id,
                                    machine: cfg.machine.clone(),
                                    project,
                                    place: String::new(),
                                    ok: false,
                                    detail: format!("{e:#}"),
                                }
                            }
                        };
                        hub_link::send_json(&mut ws, &answer).await?;
                    }
                    HubFrame::Roster { .. } => {}
                    HubFrame::Error { detail } => eprintln!("worker: hub: {detail}"),
                    HubFrame::Register { .. }
                    | HubFrame::Registered { .. }
                    | HubFrame::Open { .. }
                    | HubFrame::Frame { .. }
                    | HubFrame::Close { .. }
                    | HubFrame::Claimed { .. }
                    | HubFrame::Notify { .. }
                    | HubFrame::Seen { .. }
                    | HubFrame::Unknown => {}
                }
            }
            _ = ping.tick() => {
                if ws.send(Message::Ping(Vec::new().into())).await.is_err() {
                    return Ok(());
                }
            }
        }
    }
}

/// Resolve the checkout, cut a worktree when asked, and start a kernel
/// there that registers with the hub. Returns the project name it
/// registers under and the place it serves.
fn start_kernel(
    cfg: &HubConfig,
    dir: &Path,
    project: &str,
    claim: &str,
    isolate: bool,
) -> Result<(String, PathBuf)> {
    if project.contains('/') || project.contains("..") || project.starts_with('.') {
        bail!("project {project:?} is not a checkout name");
    }
    let checkout = dir.join(project);
    if !checkout.is_dir() {
        let have = projects_in(dir);
        bail!(
            "no checkout named {project:?} under {} on {} (have: {}). Clone it there first; the worker never syncs code.",
            dir.display(),
            cfg.machine,
            if have.is_empty() {
                "none".to_string()
            } else {
                have.join(", ")
            }
        );
    }
    let (served, place) = if isolate {
        let wt = crate::worktree::create(&checkout, claim)
            .with_context(|| format!("worktree of {}", checkout.display()))?;
        (format!("{project}--{claim}"), wt.path)
    } else {
        (project.to_string(), checkout)
    };
    std::fs::create_dir_all(place.join(".arbos"))?;
    if let Some(port) = live_port(&place) {
        // A kernel already serves it (started by hand, or an earlier
        // claim); it registered itself when it started with --hub.
        eprintln!("worker: {} already served on port {port}", place.display());
        return Ok((served, place));
    }
    // Not `current_exe()` alone: on arboslife the daemon had outlived its
    // own binary (replaced by an update) and every spawn was refused
    // with ENOENT for two days (JB-6).
    let chosen = crate::binary::kernel_binary().context("locate arbos-kernel")?;
    if let Some(note) = &chosen.note {
        eprintln!("worker: {note}");
    }
    let log = std::fs::File::create(place.join(".arbos").join("kernel.log"))?;
    let mut cmd = Command::new(&chosen.path);
    cmd.arg("serve")
        .arg(&place)
        .arg("--hub")
        .arg(&cfg.url)
        .arg("--machine")
        .arg(&cfg.machine)
        .arg("--project")
        .arg(&served);
    // A worktree kernel exists for one claim: once the claiming kernel's
    // channel is gone and nothing runs, it exits (qa-038). The checkout's
    // own kernel stays for the next opener.
    if isolate {
        cmd.arg("--leash").arg(WORKTREE_LEASH);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    // Its own process group: the worker stopping does not stop the kernel,
    // and a kernel's job timeouts never reach the worker.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let _child = cmd.spawn().context("start arbos-kernel serve")?;
    let deadline = std::time::Instant::now() + KERNEL_READY;
    while std::time::Instant::now() < deadline {
        if live_port(&place).is_some() {
            return Ok((served, place));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    bail!(
        "the kernel for {} wrote no kernel.json within {KERNEL_READY:?}; see {}/.arbos/kernel.log",
        place.display(),
        place.display()
    )
}

/// The loopback port of a live kernel for `place`, if any.
fn live_port(place: &Path) -> Option<u16> {
    #[derive(serde::Deserialize)]
    struct KernelJson {
        url: String,
        pid: u32,
    }
    let text = std::fs::read_to_string(place.join(".arbos").join("kernel.json")).ok()?;
    let info: KernelJson = serde_json::from_str(&text).ok()?;
    if !Path::new(&format!("/proc/{}", info.pid)).exists()
        && !Command::new("kill")
            .args(["-0", &info.pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    {
        return None;
    }
    let addr = info.url.strip_prefix("tcp://")?;
    let port = addr.rsplit(':').next()?.parse::<u16>().ok()?;
    std::net::TcpStream::connect_timeout(&addr.parse().ok()?, Duration::from_secs(2)).ok()?;
    Some(port)
}
