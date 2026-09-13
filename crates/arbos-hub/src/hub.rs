//! The registry and the three routes.
//!
//! - `/register`: a kernel or worker connects outbound and stays. Its
//!   socket carries numbered channels, one per attached client.
//! - `/attach/<machine>[/<project>]`: a client is joined to that kernel.
//!   It sees exactly what a direct WebSocket to the kernel would show.
//! - `/claim/<machine>`: a client asks the machine's worker to start a
//!   kernel for a project; when that kernel registers, the same socket
//!   becomes an attach to it.
//!
//! Everything the hub knows about a machine is in one map, rebuilt from
//! the live sockets: a machine that disconnects is gone from the roster
//! at once, and every registrant gets the new roster.

use anyhow::{Result, bail};
use arbos_core::hub::{HubFrame, MachineInfo, ProjectInfo, RegistrantKind};
use arbos_core::wire::Frame;
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

use crate::auth::Identity;

/// How long a registrant has to send its `register` frame, and a claimer
/// its `claim`.
const FIRST_FRAME: Duration = Duration::from_secs(10);
/// How long a worker has to answer a claim (it may sync or cut a worktree).
const CLAIM_WAIT: Duration = Duration::from_secs(120);
/// After the worker says the kernel started, how long until it registers.
const KERNEL_WAIT: Duration = Duration::from_secs(90);
/// Keepalive toward registrants, so a tunnel in the middle keeps the
/// socket open while nothing else happens.
const PING_EVERY: Duration = Duration::from_secs(30);

type Ws = WebSocketStream<TcpStream>;

/// One connected kernel or worker.
pub struct Registrant {
    pub id: u64,
    pub machine: String,
    pub project: Option<String>,
    pub place: Option<String>,
    to_socket: mpsc::UnboundedSender<HubFrame>,
    chans: Mutex<HashMap<u64, mpsc::UnboundedSender<Frame>>>,
    next_chan: AtomicU64,
}

impl Registrant {
    fn send(&self, f: HubFrame) -> bool {
        self.to_socket.send(f).is_ok()
    }

    /// A new channel toward this kernel for one client; frames the kernel
    /// sends on it arrive at `to_client`.
    fn open(&self, to_client: mpsc::UnboundedSender<Frame>, who: &str, role: &str) -> u64 {
        let chan = self.next_chan.fetch_add(1, Ordering::Relaxed);
        self.chans.lock().unwrap().insert(chan, to_client);
        self.send(HubFrame::Open {
            chan,
            who: who.to_string(),
            role: role.to_string(),
        });
        chan
    }

    fn close(&self, chan: u64, reason: &str) {
        if self.chans.lock().unwrap().remove(&chan).is_some() {
            self.send(HubFrame::Close {
                chan,
                reason: reason.to_string(),
            });
        }
    }
}

#[derive(Default)]
struct MachineEntry {
    user: String,
    host: String,
    labels: Vec<String>,
    capabilities: Vec<String>,
    version: String,
    since: i64,
    worker: Option<Arc<Registrant>>,
    /// Checkouts the worker offered.
    worker_projects: Vec<String>,
    /// Live kernels by project name.
    kernels: HashMap<String, Arc<Registrant>>,
}

impl MachineEntry {
    fn is_empty(&self) -> bool {
        self.worker.is_none() && self.kernels.is_empty()
    }

    fn info(&self, name: &str) -> MachineInfo {
        let mut projects: Vec<ProjectInfo> = self
            .kernels
            .iter()
            .map(|(p, r)| ProjectInfo {
                name: p.clone(),
                place: r.place.clone().unwrap_or_default(),
                live: true,
            })
            .collect();
        for p in &self.worker_projects {
            if !self.kernels.contains_key(p) {
                projects.push(ProjectInfo {
                    name: p.clone(),
                    place: String::new(),
                    live: false,
                });
            }
        }
        projects.sort_by(|a, b| a.name.cmp(&b.name));
        MachineInfo {
            name: name.to_string(),
            user: self.user.clone(),
            host: self.host.clone(),
            labels: self.labels.clone(),
            capabilities: self.capabilities.clone(),
            version: self.version.clone(),
            worker: self.worker.is_some(),
            projects,
            since: self.since,
        }
    }
}

#[derive(Default)]
struct Inner {
    next_id: u64,
    machines: HashMap<String, MachineEntry>,
    /// Claims waiting for the worker's answer, by claim id.
    claims: HashMap<String, oneshot::Sender<HubFrame>>,
    /// Bumped on every registration change; kernel waits poll it.
    generation: u64,
}

#[derive(Default)]
pub struct Hub {
    inner: Mutex<Inner>,
}

impl Hub {
    pub fn roster(&self) -> Vec<MachineInfo> {
        let g = self.inner.lock().unwrap();
        let mut out: Vec<MachineInfo> = g.machines.iter().map(|(n, e)| e.info(n)).collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Every registrant hears the roster after a change.
    fn broadcast_roster(&self) {
        let roster = self.roster();
        let targets: Vec<Arc<Registrant>> = {
            let g = self.inner.lock().unwrap();
            g.machines
                .values()
                .flat_map(|e| e.worker.iter().cloned().chain(e.kernels.values().cloned()))
                .collect()
        };
        for r in targets {
            r.send(HubFrame::Roster {
                machines: roster.clone(),
            });
        }
    }

    fn kernel(&self, machine: &str, project: Option<&str>) -> Result<Arc<Registrant>> {
        let g = self.inner.lock().unwrap();
        let Some(entry) = g
            .machines
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(machine))
            .map(|(_, e)| e)
        else {
            let known: Vec<&String> = g.machines.keys().collect();
            bail!(
                "no machine named {machine:?} is registered (known: {})",
                if known.is_empty() {
                    "none".to_string()
                } else {
                    known
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            );
        };
        match project {
            Some(p) => entry
                .kernels
                .get(p)
                .cloned()
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "{machine} has no kernel serving {p:?} (live: {}){}",
                        list(entry.kernels.keys()),
                        if entry.worker.is_some() {
                            "; it has a worker, so a claim can start one"
                        } else {
                            ""
                        }
                    )
                }),
            None => match entry.kernels.len() {
                1 => Ok(entry.kernels.values().next().unwrap().clone()),
                0 => bail!(
                    "{machine} has no live kernel{}",
                    if entry.worker.is_some() {
                        "; it has a worker, so a claim can start one"
                    } else {
                        ""
                    }
                ),
                _ => bail!(
                    "{machine} serves several projects; name one: {}",
                    list(entry.kernels.keys())
                ),
            },
        }
    }

    fn worker(&self, machine: &str) -> Result<Arc<Registrant>> {
        let g = self.inner.lock().unwrap();
        g.machines
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(machine))
            .and_then(|(_, e)| e.worker.clone())
            .ok_or_else(|| anyhow::anyhow!("no worker is connected for machine {machine:?}"))
    }
}

fn list<'a>(it: impl Iterator<Item = &'a String>) -> String {
    let mut v: Vec<&str> = it.map(String::as_str).collect();
    v.sort_unstable();
    if v.is_empty() {
        "none".into()
    } else {
        v.join(", ")
    }
}

async fn next_text(ws: &mut Ws) -> Option<String> {
    loop {
        match ws.next().await? {
            Ok(Message::Text(t)) => return Some(t.to_string()),
            Ok(Message::Binary(b)) => return Some(String::from_utf8_lossy(&b).to_string()),
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => continue,
        }
    }
}

async fn send_json<T: serde::Serialize>(ws: &mut Ws, v: &T) -> bool {
    match serde_json::to_string(v) {
        Ok(s) => ws.send(Message::Text(s.into())).await.is_ok(),
        Err(_) => false,
    }
}

// ── /register ───────────────────────────────────────────────────────────

pub async fn register(hub: Arc<Hub>, mut ws: Ws, who: Identity, peer: String) {
    let Identity::Machine(token_machine) = who else {
        let _ = send_json(
            &mut ws,
            &HubFrame::Error {
                detail: "only a machine token may register".into(),
            },
        )
        .await;
        return;
    };
    let first = tokio::time::timeout(FIRST_FRAME, next_text(&mut ws))
        .await
        .ok()
        .flatten();
    let Some(HubFrame::Register {
        machine,
        kind,
        user,
        host,
        project,
        place,
        projects,
        labels,
        capabilities,
        version,
        protocol,
    }) = first.and_then(|l| serde_json::from_str::<HubFrame>(&l).ok())
    else {
        let _ = send_json(
            &mut ws,
            &HubFrame::Error {
                detail: "first frame must be register".into(),
            },
        )
        .await;
        return;
    };
    if !machine.eq_ignore_ascii_case(&token_machine) {
        let _ = send_json(
            &mut ws,
            &HubFrame::Error {
                detail: format!("this token registers {token_machine:?}, not {machine:?}"),
            },
        )
        .await;
        return;
    }
    if protocol > arbos_core::hub::HUB_PROTOCOL {
        eprintln!(
            "hub: {machine} speaks hub protocol {protocol}, this hub {}; carrying on",
            arbos_core::hub::HUB_PROTOCOL
        );
    }
    let machine = token_machine;
    if kind == RegistrantKind::Kernel && project.is_none() {
        let _ = send_json(
            &mut ws,
            &HubFrame::Error {
                detail: "a kernel must register a project".into(),
            },
        )
        .await;
        return;
    }
    let (to_socket, mut from_hub) = mpsc::unbounded_channel::<HubFrame>();
    let reg = {
        let mut g = hub.inner.lock().unwrap();
        g.next_id += 1;
        let reg = Arc::new(Registrant {
            id: g.next_id,
            machine: machine.clone(),
            project: project.clone(),
            place: place.clone(),
            to_socket,
            chans: Mutex::new(HashMap::new()),
            next_chan: AtomicU64::new(1),
        });
        let entry = g.machines.entry(machine.clone()).or_default();
        if entry.is_empty() {
            entry.since = arbos_core::now_ms();
        }
        // A machine is described by every registrant on it: the worker
        // says `gpu`, a kernel says the version. Words add up; none erase.
        if !user.is_empty() {
            entry.user = user;
        }
        if !host.is_empty() {
            entry.host = host;
        }
        for l in labels {
            if !entry.labels.contains(&l) {
                entry.labels.push(l);
            }
        }
        for c in capabilities {
            if !entry.capabilities.contains(&c) {
                entry.capabilities.push(c);
            }
        }
        if !version.is_empty() {
            entry.version = version;
        }
        match kind {
            RegistrantKind::Worker => {
                entry.worker = Some(Arc::clone(&reg));
                entry.worker_projects = projects;
            }
            RegistrantKind::Kernel => {
                entry
                    .kernels
                    .insert(project.clone().unwrap_or_default(), Arc::clone(&reg));
            }
        }
        g.generation += 1;
        reg
    };
    eprintln!(
        "hub: registered {} {} project={} from {peer}",
        kind.as_str(),
        machine,
        project.as_deref().unwrap_or("-")
    );
    let _ = send_json(
        &mut ws,
        &HubFrame::Registered {
            machine: machine.clone(),
            id: reg.id,
        },
    )
    .await;
    hub.broadcast_roster();

    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    loop {
        tokio::select! {
            out = from_hub.recv() => {
                let Some(f) = out else { break };
                if !send_json(&mut ws, &f).await {
                    break;
                }
            }
            line = next_text(&mut ws) => {
                let Some(line) = line else { break };
                let Ok(f) = serde_json::from_str::<HubFrame>(&line) else { continue };
                match f {
                    HubFrame::Frame { chan, frame } => {
                        let tx = reg.chans.lock().unwrap().get(&chan).cloned();
                        match tx {
                            Some(tx) => {
                                if tx.send(frame).is_err() {
                                    reg.close(chan, "client gone");
                                }
                            }
                            None => {
                                reg.send(HubFrame::Close { chan, reason: "no such channel".into() });
                            }
                        }
                    }
                    HubFrame::Close { chan, .. } => {
                        reg.chans.lock().unwrap().remove(&chan);
                    }
                    HubFrame::Claimed { .. } => {
                        let id = match &f {
                            HubFrame::Claimed { id, .. } => id.clone(),
                            _ => unreachable!(),
                        };
                        let waiter = hub.inner.lock().unwrap().claims.remove(&id);
                        if let Some(tx) = waiter {
                            let _ = tx.send(f);
                        }
                    }
                    // A registrant sends nothing else; a second register is nothing.
                    HubFrame::Register { .. }
                    | HubFrame::Registered { .. }
                    | HubFrame::Roster { .. }
                    | HubFrame::Open { .. }
                    | HubFrame::Claim { .. }
                    | HubFrame::Error { .. }
                    | HubFrame::Unknown => {}
                }
            }
            _ = ping.tick() => {
                if ws.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
        }
    }
    // Gone: every client on it sees its channel end.
    reg.chans.lock().unwrap().clear();
    {
        let mut g = hub.inner.lock().unwrap();
        if let Some(entry) = g.machines.get_mut(&machine) {
            match kind {
                RegistrantKind::Worker => {
                    if entry.worker.as_ref().is_some_and(|w| w.id == reg.id) {
                        entry.worker = None;
                        entry.worker_projects.clear();
                    }
                }
                RegistrantKind::Kernel => {
                    let key = project.clone().unwrap_or_default();
                    if entry.kernels.get(&key).is_some_and(|k| k.id == reg.id) {
                        entry.kernels.remove(&key);
                    }
                }
            }
            if entry.is_empty() {
                g.machines.remove(&machine);
            }
        }
        g.generation += 1;
    }
    eprintln!(
        "hub: unregistered {} {} project={}",
        kind.as_str(),
        machine,
        project.as_deref().unwrap_or("-")
    );
    hub.broadcast_roster();
}

// ── /attach ─────────────────────────────────────────────────────────────

pub async fn attach(hub: Arc<Hub>, mut ws: Ws, who: Identity, machine: &str, project: Option<&str>) {
    let kernel = match hub.kernel(machine, project) {
        Ok(k) => k,
        Err(e) => {
            let _ = send_json(
                &mut ws,
                &Frame::Error {
                    agent: None,
                    detail: format!("hub: {e}"),
                },
            )
            .await;
            return;
        }
    };
    proxy(ws, who, kernel).await;
}

/// Join one client socket to one kernel channel until either side ends.
async fn proxy(mut ws: Ws, who: Identity, kernel: Arc<Registrant>) {
    let (to_client, mut from_kernel) = mpsc::unbounded_channel::<Frame>();
    let (name, role) = who.as_client();
    let chan = kernel.open(to_client, &name, &role);
    eprintln!(
        "hub: {name} ({role}) attached to {}/{} chan {chan}",
        kernel.machine,
        kernel.project.as_deref().unwrap_or("-")
    );
    loop {
        tokio::select! {
            f = from_kernel.recv() => {
                let Some(f) = f else {
                    let _ = send_json(&mut ws, &Frame::Error { agent: None, detail: format!("hub: the kernel on {} went away", kernel.machine) }).await;
                    break;
                };
                if !send_json(&mut ws, &f).await {
                    break;
                }
            }
            line = next_text(&mut ws) => {
                let Some(line) = line else { break };
                for l in line.lines().filter(|l| !l.trim().is_empty()) {
                    match serde_json::from_str::<Frame>(l) {
                        Ok(frame) => {
                            if !kernel.send(HubFrame::Frame { chan, frame }) {
                                break;
                            }
                        }
                        Err(e) => {
                            let _ = send_json(&mut ws, &Frame::Error { agent: None, detail: format!("hub: not a frame: {e}") }).await;
                        }
                    }
                }
            }
        }
    }
    kernel.close(chan, "client left");
    eprintln!("hub: {name} left {} chan {chan}", kernel.machine);
}

// ── /claim ──────────────────────────────────────────────────────────────

pub async fn claim(hub: Arc<Hub>, mut ws: Ws, who: Identity, machine: &str) {
    let (name, role) = who.as_client();
    if role == "reader" {
        let _ = send_json(
            &mut ws,
            &HubFrame::Error {
                detail: "a reader may not claim a machine".into(),
            },
        )
        .await;
        return;
    }
    let first = tokio::time::timeout(FIRST_FRAME, next_text(&mut ws))
        .await
        .ok()
        .flatten();
    let Some(HubFrame::Claim {
        project,
        isolate,
        from,
        ..
    }) = first.and_then(|l| serde_json::from_str::<HubFrame>(&l).ok())
    else {
        let _ = send_json(
            &mut ws,
            &HubFrame::Error {
                detail: "first frame must be claim".into(),
            },
        )
        .await;
        return;
    };
    let worker = match hub.worker(machine) {
        Ok(w) => w,
        Err(e) => {
            let _ = send_json(
                &mut ws,
                &HubFrame::Claimed {
                    id: String::new(),
                    machine: machine.to_string(),
                    project,
                    place: String::new(),
                    ok: false,
                    detail: e.to_string(),
                },
            )
            .await;
            return;
        }
    };
    let id = format!(
        "c{}-{}",
        arbos_core::now_ms() % 1_000_000,
        worker.next_chan.fetch_add(1, Ordering::Relaxed)
    );
    let (tx, rx) = oneshot::channel();
    hub.inner.lock().unwrap().claims.insert(id.clone(), tx);
    let from = if from.is_empty() { name.clone() } else { from };
    eprintln!("hub: {name} claims {} for {project} (isolate={isolate}) id {id}", worker.machine);
    if !worker.send(HubFrame::Claim {
        id: id.clone(),
        project: project.clone(),
        isolate,
        from,
    }) {
        hub.inner.lock().unwrap().claims.remove(&id);
        let _ = send_json(
            &mut ws,
            &HubFrame::Claimed {
                id,
                machine: machine.to_string(),
                project,
                place: String::new(),
                ok: false,
                detail: "the worker's socket closed".into(),
            },
        )
        .await;
        return;
    }
    let answer = match tokio::time::timeout(CLAIM_WAIT, rx).await {
        Ok(Ok(f)) => f,
        _ => {
            hub.inner.lock().unwrap().claims.remove(&id);
            HubFrame::Claimed {
                id: id.clone(),
                machine: machine.to_string(),
                project: project.clone(),
                place: String::new(),
                ok: false,
                detail: format!("the worker on {machine} did not answer within {CLAIM_WAIT:?}"),
            }
        }
    };
    let (ok, served) = match &answer {
        HubFrame::Claimed { ok, project, .. } => (*ok, project.clone()),
        _ => (false, project.clone()),
    };
    if !ok {
        let _ = send_json(&mut ws, &answer).await;
        return;
    }
    // The worker started a kernel; it registers under `served` when its
    // socket to the hub is up.
    let deadline = tokio::time::Instant::now() + KERNEL_WAIT;
    let kernel = loop {
        if let Ok(k) = hub.kernel(machine, Some(&served)) {
            break Some(k);
        }
        if tokio::time::Instant::now() >= deadline {
            break None;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    let Some(kernel) = kernel else {
        let _ = send_json(
            &mut ws,
            &HubFrame::Claimed {
                id,
                machine: machine.to_string(),
                project: served.clone(),
                place: String::new(),
                ok: false,
                detail: format!(
                    "the worker started a kernel for {served} but it did not register within {KERNEL_WAIT:?}"
                ),
            },
        )
        .await;
        return;
    };
    if !send_json(&mut ws, &answer).await {
        return;
    }
    proxy(ws, who, kernel).await;
}
