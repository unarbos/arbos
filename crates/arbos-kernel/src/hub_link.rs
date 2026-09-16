//! The kernel's side of the mesh: one outbound WebSocket to `arbos-hub`.
//!
//! `arbos-kernel serve --hub wss://… --machine <name>` registers this
//! kernel under the machine name and keeps the socket open. Clients the
//! hub admits arrive as numbered channels on that socket; each becomes a
//! virtual attach client, served by the same code as a TCP peer. The hub
//! also pushes its roster, which lands in `.arbos/machines/` so agents
//! (and the panel) discover other machines by reading files.
//!
//! The same module dials the hub as a *client*: `spawn host=` claims a
//! worker, `say to=<machine>/<agent>` delivers a message, and
//! `arbos-kernel attach --hub <machine>` follows a kernel by name.

use anyhow::{Context, Result, bail};
use arbos_core::hub::{HUB_PROTOCOL, HubConfig, HubFrame, RegistrantKind};
use arbos_core::{Place, wire::Frame};
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::access::{Identity, Role};
use crate::attach::HubChannel;
use crate::hooks::KernelHooks;
use crate::klog;

/// Env vars the `serve` flags become.
pub const URL_ENV: &str = "ARBOS_HUB";
pub const MACHINE_ENV: &str = "ARBOS_HUB_MACHINE";
pub const PROJECT_ENV: &str = "ARBOS_HUB_PROJECT";

/// How long a dial may take before it counts as failed.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// Ping the hub so an idle tunnel keeps the socket.
const PING_EVERY: Duration = Duration::from_secs(30);

pub type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The hub this kernel should register with, from the environment the
/// `serve` flags set and `~/.config/arbos/hub.toml`.
pub fn config_from_env() -> Result<Option<HubConfig>> {
    let url = std::env::var(URL_ENV).ok();
    let machine = std::env::var(MACHINE_ENV).ok();
    HubConfig::resolve(url.as_deref(), machine.as_deref())
}

/// The project name this kernel registers: `--project`, else the place's
/// folder name.
pub fn project_name(place: &Place) -> String {
    std::env::var(PROJECT_ENV)
        .ok()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| {
            place
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .filter(|n| !n.is_empty())
                .unwrap_or("project")
                .to_string()
        })
}

/// Open a WebSocket to `url` with the bearer token.
pub async fn connect(url: &str, token: &str) -> Result<Ws> {
    let mut req = url
        .into_client_request()
        .with_context(|| format!("bad hub url {url:?}"))?;
    req.headers_mut().insert(
        "authorization",
        format!("Bearer {token}")
            .parse()
            .context("token is not a header value")?,
    );
    let (ws, _) = tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(req))
        .await
        .with_context(|| format!("connect {url}: timed out"))?
        .with_context(|| format!("connect {url}"))?;
    Ok(ws)
}

/// The next text message as a string, or `None` when the socket ended.
pub async fn next_text(ws: &mut Ws) -> Option<String> {
    loop {
        match ws.next().await? {
            Ok(Message::Text(t)) => return Some(t.to_string()),
            Ok(Message::Binary(b)) => return Some(String::from_utf8_lossy(&b).to_string()),
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => continue,
        }
    }
}

pub async fn send_json<T: serde::Serialize>(ws: &mut Ws, v: &T) -> Result<()> {
    let s = serde_json::to_string(v)?;
    ws.send(Message::Text(s.into())).await?;
    Ok(())
}

/// What every registrant says about itself, plus the words it was given.
pub fn labels(extra: &[String]) -> Vec<String> {
    let mut out = vec![
        std::env::consts::OS.to_string(),
        std::env::consts::ARCH.to_string(),
    ];
    for l in extra {
        let l = l.trim();
        if !l.is_empty() && !out.iter().any(|x| x == l) {
            out.push(l.to_string());
        }
    }
    out
}

pub fn user_name() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_default()
}

pub fn host_name() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_default()
}

/// Send `register` and wait for `registered`. Returns the id the hub gave.
pub async fn register(
    ws: &mut Ws,
    cfg: &HubConfig,
    kind: RegistrantKind,
    project: Option<String>,
    place: Option<String>,
    projects: Vec<String>,
    extra_labels: &[String],
    capabilities: Vec<String>,
    identities: std::collections::BTreeMap<String, arbos_core::project::ProjectIdentity>,
    shares: std::collections::BTreeMap<String, String>,
) -> Result<u64> {
    send_json(
        ws,
        &HubFrame::Register {
            machine: cfg.machine.clone(),
            kind,
            user: user_name(),
            host: host_name(),
            project,
            place,
            projects,
            identities,
            shares,
            labels: labels(extra_labels),
            capabilities,
            version: klog::version().to_string(),
            git_sha: klog::git_sha().to_string(),
            built_at: klog::built_at().to_string(),
            protocol: HUB_PROTOCOL,
        },
    )
    .await?;
    let answer = tokio::time::timeout(CONNECT_TIMEOUT, next_text(ws))
        .await
        .context("hub did not answer the registration")?
        .context("hub closed before answering the registration")?;
    match serde_json::from_str::<HubFrame>(&answer) {
        Ok(HubFrame::Registered { id, .. }) => Ok(id),
        Ok(HubFrame::Error { detail }) => bail!("hub refused: {detail}"),
        Ok(other) => bail!("hub answered {other:?} instead of registered"),
        Err(e) => bail!("hub answered something that is not a frame: {e}"),
    }
}

/// Retry delays for a lost hub: 2, 4, 8, 16, 32, then 60 s, for ever.
pub fn backoff(attempt: u32) -> Duration {
    Duration::from_secs(match attempt {
        0 => 2,
        1 => 4,
        2 => 8,
        3 => 16,
        4 => 32,
        _ => 60,
    })
}

/// The node this kernel is on the hub, `(machine, project)`, once the link
/// has started; `None` on a kernel with no hub. Read for `hello.store`
/// and to tell a local address from a remote one.
static SELF_NODE: std::sync::OnceLock<(String, String)> = std::sync::OnceLock::new();

pub fn self_node() -> Option<(&'static str, &'static str)> {
    SELF_NODE.get().map(|(m, p)| (m.as_str(), p.as_str()))
}

/// This node's store as every node addresses it: `arbos://<machine>/<project>/`.
pub fn self_store() -> Option<arbos_core::hub::StoreAddress> {
    self_node().map(|(m, p)| arbos_core::hub::StoreAddress::root(m, p))
}

/// Register this kernel with the hub and keep it registered. Returns at
/// once; the link lives in its own task for the life of the kernel.
pub fn start(
    place: Place,
    hooks: Arc<KernelHooks>,
    frames_in: mpsc::UnboundedSender<Frame>,
    cfg: HubConfig,
    project: String,
) {
    let _ = SELF_NODE.set((cfg.machine.clone(), project.clone()));
    tokio::spawn(async move {
        let mut attempt = 0u32;
        loop {
            match session(&place, &hooks, &frames_in, &cfg, &project).await {
                Ok(()) => {
                    attempt = 0;
                    klog::warn(
                        "hub_lost",
                        None,
                        format!("{}: socket closed; reconnecting", cfg.url),
                    );
                }
                Err(e) => {
                    klog::warn(
                        "hub_error",
                        None,
                        format!("{}: {e:#}; retry in {:?}", cfg.url, backoff(attempt)),
                    );
                }
            }
            tokio::time::sleep(backoff(attempt)).await;
            attempt = attempt.saturating_add(1);
        }
    });
}

/// One connected session: register, then serve channels until the socket ends.
async fn session(
    place: &Place,
    hooks: &Arc<KernelHooks>,
    frames_in: &mpsc::UnboundedSender<Frame>,
    cfg: &HubConfig,
    project: &str,
) -> Result<()> {
    let token = cfg.token()?;
    let mut ws = connect(&cfg.register_url(), &token).await?;
    let mut identities = std::collections::BTreeMap::new();
    identities.insert(project.to_string(), arbos_core::project::identity(place));
    let mut shares = std::collections::BTreeMap::new();
    if let Some(mode) = arbos_core::project::share_mode_set(place) {
        shares.insert(project.to_string(), mode.to_string());
    }
    let id = register(
        &mut ws,
        cfg,
        RegistrantKind::Kernel,
        Some(project.to_string()),
        Some(place.path.display().to_string()),
        Vec::new(),
        &[],
        Vec::new(),
        identities,
        shares,
    )
    .await?;
    klog::info(
        "hub_registered",
        None,
        format!(
            "hub={} machine={} project={project} id={id}",
            cfg.url, cfg.machine
        ),
    );
    let (to_hub, mut from_clients) = mpsc::unbounded_channel::<HubFrame>();
    // Notifications and seen marks ride the same socket, so the hub can
    // push to a phone with no client attached.
    *hooks.hub_out.lock().unwrap() = Some((project.to_string(), to_hub.clone()));
    let mut chans: HashMap<u64, (mpsc::UnboundedSender<String>, Arc<AtomicBool>)> = HashMap::new();
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    let result = loop {
        tokio::select! {
            out = from_clients.recv() => {
                let Some(f) = out else { break Ok(()) };
                if let Err(e) = send_json(&mut ws, &f).await {
                    break Err(e);
                }
            }
            line = next_text(&mut ws) => {
                let Some(line) = line else { break Ok(()) };
                let Ok(f) = serde_json::from_str::<HubFrame>(&line) else { continue };
                match f {
                    HubFrame::Open { chan, who, role } => {
                        let role = match role.as_str() {
                            "owner" => Role::Owner,
                            "reader" => Role::Reader,
                            _ => Role::Writer,
                        };
                        let (tx, rx) = mpsc::unbounded_channel::<String>();
                        let open = Arc::new(AtomicBool::new(true));
                        chans.insert(chan, (tx, Arc::clone(&open)));
                        let (r, w) = HubChannel { chan, to_hub: to_hub.clone(), open }.split(rx);
                        klog::info("hub_attach", None, format!("chan={chan} who={who} role={}", role.as_str()));
                        tokio::spawn(crate::serve::serve_client(
                            r,
                            w,
                            Identity { name: format!("hub:{who}"), role },
                            place.clone(),
                            Arc::clone(hooks),
                            frames_in.clone(),
                        ));
                    }
                    HubFrame::Frame { chan, frame } => {
                        // The client's JSON as it was sent; this kernel's own
                        // parser reads it in the channel's read loop.
                        if let Some((tx, _)) = chans.get(&chan) {
                            let line = frame.to_string();
                            if tx.send(line).is_err() {
                                chans.remove(&chan);
                            }
                        }
                    }
                    HubFrame::Close { chan, .. } => {
                        if let Some((_, open)) = chans.remove(&chan) {
                            open.store(false, Ordering::Relaxed);
                        }
                    }
                    HubFrame::Roster { machines } => {
                        if let Err(e) = arbos_core::hub::write_roster(place, &cfg.url, &machines) {
                            klog::warn("hub_roster", None, format!("write .arbos/machines: {e:#}"));
                        }
                    }
                    HubFrame::Error { detail } => {
                        klog::warn("hub_error", None, detail);
                    }
                    // Not for a kernel; a worker gets claims.
                    HubFrame::Register { .. }
                    | HubFrame::Registered { .. }
                    | HubFrame::Claim { .. }
                    | HubFrame::Claimed { .. }
                    | HubFrame::Notify { .. }
                    | HubFrame::Seen { .. }
                    | HubFrame::Unknown => {}
                }
            }
            _ = ping.tick() => {
                if ws.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break Ok(());
                }
            }
        }
    };
    // Every virtual client ends with the link.
    for (_, (_, open)) in chans.drain() {
        open.store(false, Ordering::Relaxed);
    }
    *hooks.hub_out.lock().unwrap() = None;
    result
}

// ── the kernel as a hub client ──────────────────────────────────────────

/// Attach to `machine`'s kernel (for `project`, when it serves several)
/// and wait for its `hello`. The socket then speaks the plain attach wire.
pub async fn attach(cfg: &HubConfig, machine: &str, project: Option<&str>) -> Result<Ws> {
    let token = cfg.token()?;
    let mut ws = connect(&cfg.attach_url(machine, project), &token).await?;
    let first = tokio::time::timeout(CONNECT_TIMEOUT, next_text(&mut ws))
        .await
        .with_context(|| format!("no answer from {machine} through the hub"))?
        .with_context(|| format!("the hub closed the attach to {machine}"))?;
    match serde_json::from_str::<Frame>(&first) {
        Ok(Frame::Hello { .. }) => Ok(ws),
        Ok(Frame::Error { detail, .. }) => bail!("{detail}"),
        Ok(other) => bail!("{machine} answered {other:?} instead of hello"),
        Err(e) => bail!("{machine} answered something that is not a frame: {e}"),
    }
}

/// Deliver one message to an agent on another machine. Returns a receipt.
pub async fn deliver(
    cfg: &HubConfig,
    machine: &str,
    project: Option<&str>,
    agent: &str,
    from: &str,
    text: &str,
) -> Result<String> {
    let mut ws = attach(cfg, machine, project).await?;
    send_json(
        &mut ws,
        &Frame::User {
            agent: agent.to_string(),
            text: format!("[{from}] {text}"),
            steer: false,
            attachments: vec![],
            channel: String::new(),
            device: String::new(),
            model: String::new(),
        },
    )
    .await?;
    // The kernel answers a bad agent id with an error naming it; a good
    // one is silent, so a short wait with no error is the receipt.
    let deadline = tokio::time::Instant::now() + Duration::from_millis(1500);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            break;
        }
        match tokio::time::timeout(left, next_text(&mut ws)).await {
            Ok(Some(line)) => {
                if let Ok(Frame::Error {
                    agent: Some(a),
                    detail,
                }) = serde_json::from_str::<Frame>(&line)
                    && a == agent
                {
                    let _ = ws.close(None).await;
                    bail!("{machine} refused: {detail}");
                }
            }
            _ => break,
        }
    }
    let _ = ws.close(None).await;
    Ok(format!(
        "Delivered to {agent} on {machine}{} through the hub as a message from {from}; it runs a turn there. Replies come back the same way (say to={}/{from}).",
        project.map(|p| format!(" ({p})")).unwrap_or_default(),
        cfg.machine
    ))
}

// ── another node's store, by address ─────────────────────────────────────

/// How long one read, list, or write of a peer's store may take, dial
/// included. Past it the tool fails with the address and the reason;
/// nothing stale or empty is returned in its place.
const STORE_TIMEOUT: Duration = Duration::from_secs(10);

/// Attach to the node an address names and run `ask` on the socket, or
/// read the file here when the address is this very node (the fast path).
async fn with_store<T, F>(address: &str, local: F, ask: Frame) -> Result<T>
where
    F: FnOnce(Frame) -> Option<Frame>,
    T: FromReply,
{
    let addr = arbos_core::hub::StoreAddress::parse(address)?;
    if let Some((m, p)) = self_node()
        && addr.is_node(m, p)
    {
        let reply = local(ask).with_context(|| format!("{addr}: no reply from this kernel"))?;
        return T::from_reply(reply);
    }
    let cfg = config_from_env()?.with_context(|| format!("{addr}: {}", arbos_engine::NO_MESH))?;
    let work = async {
        // The hub's own refusal (no such machine, not shared with you) is
        // the answer; anything else is the road being down.
        let mut ws = match attach(&cfg, &addr.machine, Some(&addr.project)).await {
            Ok(ws) => ws,
            Err(e) if e.to_string().starts_with("hub:") => bail!("{addr}: {e}"),
            Err(e) => {
                return Err(e.context(format!(
                    "{addr}: {} is not reachable through the hub; nothing was read or written",
                    addr.machine
                )));
            }
        };
        let want = match &ask {
            Frame::Read { path } | Frame::List { path } | Frame::Put { path, .. } => path.clone(),
            Frame::Tail { path, .. } => path.clone(),
            _ => String::new(),
        };
        send_json(&mut ws, &ask).await?;
        loop {
            let Some(line) = next_text(&mut ws).await else {
                bail!("{addr}: the node closed the connection before answering");
            };
            let Ok(frame) = serde_json::from_str::<Frame>(&line) else {
                continue;
            };
            let is_answer = match &frame {
                Frame::File { path, .. }
                | Frame::Listing { path, .. }
                | Frame::Written { path, .. }
                | Frame::Chunk { path, .. } => *path == want,
                // The hub's own refusal, or the kernel refusing this
                // client's role ("a reader client may not send put"): that
                // is the answer, in the words the user should read. Other
                // errors are broadcast to every client and are not ours.
                Frame::Error {
                    agent: None,
                    detail,
                } if detail.starts_with("hub:") || detail.contains("may not send") => {
                    bail!("{addr}: {detail}")
                }
                _ => false,
            };
            if is_answer {
                let _ = ws.close(None).await;
                return T::from_reply(frame);
            }
        }
    };
    match tokio::time::timeout(STORE_TIMEOUT, work).await {
        Ok(r) => r,
        Err(_) => bail!(
            "{addr}: no answer from {} within {STORE_TIMEOUT:?} (hub {}); nothing was read or written",
            addr.machine,
            cfg.url
        ),
    }
}

/// `read` by address: the file's text, or why not.
pub async fn store_read(place: &Place, address: &str) -> Result<arbos_engine::StoreFile> {
    let addr = arbos_core::hub::StoreAddress::parse(address)?;
    let place2 = place.clone();
    with_store::<arbos_engine::StoreFile, _>(
        address,
        move |f| crate::files::handle(&place2, f),
        Frame::Read { path: addr.path },
    )
    .await
}

/// `ls` by address.
pub async fn store_list(place: &Place, address: &str) -> Result<Vec<arbos_core::wire::Entry>> {
    let addr = arbos_core::hub::StoreAddress::parse(address)?;
    let place2 = place.clone();
    let listing = with_store::<Listing, _>(
        address,
        move |f| crate::files::handle(&place2, f),
        Frame::List { path: addr.path },
    )
    .await?;
    Ok(listing.0)
}

/// `write` by address, with the compare-and-swap hash when the caller
/// read the file first.
pub async fn store_write(
    place: &Place,
    address: &str,
    text: String,
    base_hash: Option<String>,
) -> Result<arbos_engine::StoreWritten> {
    let addr = arbos_core::hub::StoreAddress::parse(address)?;
    let place2 = place.clone();
    with_store::<arbos_engine::StoreWritten, _>(
        address,
        move |f| crate::files::handle(&place2, f),
        Frame::Put {
            path: addr.path,
            text,
            data: None,
            base_hash,
        },
    )
    .await
}

/// The kernel's refusal names the path itself; do not say it twice.
fn name_once(path: &str, error: &str) -> String {
    if error.starts_with(&format!("{path}:")) || error.starts_with(&format!("{path} ")) {
        error.to_string()
    } else {
        format!("{path}: {error}")
    }
}

/// A store reply frame as the value a tool wants.
trait FromReply: Sized {
    fn from_reply(f: Frame) -> Result<Self>;
}

struct Listing(Vec<arbos_core::wire::Entry>);

impl FromReply for Listing {
    fn from_reply(f: Frame) -> Result<Self> {
        match f {
            Frame::Listing {
                path,
                entries,
                error,
            } => match error {
                Some(e) => bail!("{}", name_once(&path, &e)),
                None => Ok(Listing(entries)),
            },
            other => bail!("expected a listing, got {other:?}"),
        }
    }
}

impl FromReply for arbos_engine::StoreFile {
    fn from_reply(f: Frame) -> Result<Self> {
        match f {
            Frame::File {
                path,
                text,
                size,
                truncated,
                error,
            } => match error {
                Some(e) => bail!("{}", name_once(&path, &e)),
                None => Ok(arbos_engine::StoreFile {
                    hash: arbos_core::hub::content_hash(text.as_bytes()),
                    text,
                    size,
                    truncated,
                }),
            },
            other => bail!("expected a file, got {other:?}"),
        }
    }
}

impl FromReply for arbos_engine::StoreWritten {
    fn from_reply(f: Frame) -> Result<Self> {
        match f {
            Frame::Written {
                path,
                size,
                hash,
                error,
            } => match error {
                Some(e) => bail!("{}", name_once(&path, &e)),
                None => Ok(arbos_engine::StoreWritten { size, hash }),
            },
            other => bail!("expected a written, got {other:?}"),
        }
    }
}
