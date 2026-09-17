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
    /// The `user` of the token it registered with.
    pub user: String,
    pub project: Option<String>,
    pub place: Option<String>,
    to_socket: mpsc::UnboundedSender<HubFrame>,
    /// One client's outbound side per channel: the kernel's frames as the
    /// JSON it sent them, passed through whole.
    chans: Mutex<HashMap<u64, mpsc::UnboundedSender<serde_json::Value>>>,
    next_chan: AtomicU64,
}

impl Registrant {
    fn send(&self, f: HubFrame) -> bool {
        self.to_socket.send(f).is_ok()
    }

    /// A new channel toward this kernel for one client; frames the kernel
    /// sends on it arrive at `to_client`.
    fn open(
        &self,
        to_client: mpsc::UnboundedSender<serde_json::Value>,
        who: &str,
        role: &str,
    ) -> u64 {
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
    git_sha: String,
    built_at: String,
    since: i64,
    worker: Option<Arc<Registrant>>,
    /// Checkouts the worker offered.
    worker_projects: Vec<String>,
    /// Live kernels by project name.
    kernels: HashMap<String, Arc<Registrant>>,
    /// Each project's face, by name, as its registrants read it.
    identities: HashMap<String, arbos_core::project::ProjectIdentity>,
    /// Each project's sharing mode (`[share] mode`), by name; absent = mesh.
    shares: HashMap<String, String>,
    /// Each project's declared kind (`kind = "service"`), by name.
    kinds: HashMap<String, String>,
    /// Whose machine this is: the `user` of the token it registered with.
    /// Its projects' stores belong to this user.
    owner_user: String,
    /// Worktree places a claim with `isolate` made, by the name their
    /// kernel registers under → the project they were cut from. A phone
    /// saw `demo--c616190-1` beside `demo` as a second project (M-12).
    worktrees: HashMap<String, String>,
}

impl MachineEntry {
    fn is_empty(&self) -> bool {
        self.worker.is_none() && self.kernels.is_empty()
    }

    /// The project a registered name is a worktree of: as the claim
    /// recorded it, or — after a hub restart forgot the claims — the
    /// `<project>--<claim>` shape the worker gives worktree places, when
    /// `<project>` is a project on this machine.
    fn worktree_of(&self, name: &str) -> Option<String> {
        if let Some(p) = self.worktrees.get(name) {
            return Some(p.clone());
        }
        let (head, tail) = name.split_once("--")?;
        let known =
            self.worker_projects.iter().any(|p| p == head) || self.kernels.contains_key(head);
        (known && !tail.is_empty()).then(|| head.to_string())
    }

    /// The sharing mode of a project here: its own, else its parent
    /// project's for a worktree, else the hub's default (`mesh` while one
    /// user holds every token, `private` once another person's joins).
    fn share_of(&self, project: &str, parent: Option<&str>, default: &str) -> String {
        self.shares
            .get(project)
            .or_else(|| parent.and_then(|pp| self.shares.get(pp)))
            .cloned()
            .unwrap_or_else(|| default.to_string())
    }

    /// This machine as `viewer` (a `(user, role)` from its token) sees
    /// it: every project carries its store address and, with a viewer,
    /// what that viewer may do there.
    fn info(&self, name: &str, viewer: Option<(&str, &str)>, default_share: &str) -> MachineInfo {
        let access = |share: &str| match viewer {
            Some((user, role)) => {
                arbos_core::hub::store_access(share, user, role, &self.owner_user).to_string()
            }
            None => String::new(),
        };
        let store = |p: &str| arbos_core::hub::StoreAddress::root(name, p).to_string();
        let mut projects: Vec<ProjectInfo> = self
            .kernels
            .iter()
            .map(|(p, r)| {
                let parent = self.worktree_of(p);
                let share = self.share_of(p, parent.as_deref(), default_share);
                ProjectInfo {
                    name: p.clone(),
                    place: r.place.clone().unwrap_or_default(),
                    live: true,
                    store: store(p),
                    access: access(&share),
                    share,
                    identity: self
                        .identities
                        .get(p)
                        .or_else(|| parent.as_ref().and_then(|pp| self.identities.get(pp)))
                        .cloned(),
                    kind: if parent.is_some() {
                        "worktree".into()
                    } else {
                        self.kinds.get(p).cloned().unwrap_or_default()
                    },
                    parent,
                }
            })
            .collect();
        for p in &self.worker_projects {
            if !self.kernels.contains_key(p) {
                let share = self.share_of(p, None, default_share);
                projects.push(ProjectInfo {
                    name: p.clone(),
                    place: String::new(),
                    live: false,
                    store: store(p),
                    access: access(&share),
                    share,
                    identity: self.identities.get(p).cloned(),
                    kind: self.kinds.get(p).cloned().unwrap_or_default(),
                    parent: None,
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
            git_sha: self.git_sha.clone(),
            built_at: self.built_at.clone(),
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

pub struct Hub {
    inner: Mutex<Inner>,
    /// The share mode of a project that sets none: from the token file's
    /// user count at start (`Auth::default_share`).
    default_share: &'static str,
    /// Device tokens and the APNs sender (`push.rs`).
    pub push: crate::push::Push,
}

impl Hub {
    pub fn new(default_share: &'static str, push: crate::push::Push) -> Self {
        Self {
            inner: Mutex::default(),
            default_share,
            push,
        }
    }

    /// The roster as `viewer` (a token's `(user, role)`) sees it: with
    /// each store's address and the viewer's access to it. `None` leaves
    /// `access` empty.
    pub fn roster_for(&self, viewer: Option<(&str, &str)>) -> Vec<MachineInfo> {
        let g = self.inner.lock().unwrap();
        let mut out: Vec<MachineInfo> = g
            .machines
            .iter()
            .map(|(n, e)| e.info(n, viewer, self.default_share))
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// What `who` may do in `machine`'s `project` store: its token role
    /// capped by the project's share mode. `none` when the machine or
    /// project is unknown, so a name that is not there reads as no access.
    pub fn access_of(&self, machine: &str, project: &str, who: &Identity) -> String {
        let g = self.inner.lock().unwrap();
        let Some(entry) = g
            .machines
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(machine))
            .map(|(_, e)| e)
        else {
            return "none".into();
        };
        let parent = entry.worktree_of(project);
        let share = entry.share_of(project, parent.as_deref(), self.default_share);
        arbos_core::hub::store_access(&share, who.user(), who.role(), &entry.owner_user).to_string()
    }

    /// Every registrant hears the roster after a change, each with its own
    /// access to every store (a machine token is an owner of its user's
    /// stores; a private project of another user reads `none`).
    fn broadcast_roster(&self) {
        let targets: Vec<Arc<Registrant>> = {
            let g = self.inner.lock().unwrap();
            g.machines
                .values()
                .flat_map(|e| e.worker.iter().cloned().chain(e.kernels.values().cloned()))
                .collect()
        };
        for r in targets {
            let machines = self.roster_for(Some((&r.user, "owner")));
            r.send(HubFrame::Roster { machines });
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
            Some(p) => entry.kernels.get(p).cloned().ok_or_else(|| {
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

/// End a refused connection so the peer reads the reason first. A tunnel
/// in front (cloudflared) drops frames an origin sends and then closes in
/// the same instant after the upgrade — the client saw a bare close and
/// never the `error`. A short pause before the close handshake lets the
/// proxy forward the text; refusals are rare, so the wait costs nothing.
async fn refuse_close(ws: &mut Ws) {
    tokio::time::sleep(Duration::from_millis(400)).await;
    let _ = ws.close(None).await;
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
    let Identity::Machine {
        name: token_machine,
        user: token_user,
    } = who
    else {
        let _ = send_json(
            &mut ws,
            &HubFrame::Error {
                detail: "only a machine token may register".into(),
            },
        )
        .await;
        refuse_close(&mut ws).await;
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
        identities,
        shares,
        kinds,
        labels,
        capabilities,
        version,
        git_sha,
        built_at,
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
        refuse_close(&mut ws).await;
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
        refuse_close(&mut ws).await;
        return;
    }
    if protocol > arbos_core::hub::HUB_PROTOCOL {
        log!(
            "{machine} speaks hub protocol {protocol}, this hub {}; carrying on",
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
        refuse_close(&mut ws).await;
        return;
    }
    let (to_socket, mut from_hub) = mpsc::unbounded_channel::<HubFrame>();
    let reg = {
        let mut g = hub.inner.lock().unwrap();
        g.next_id += 1;
        let reg = Arc::new(Registrant {
            id: g.next_id,
            machine: machine.clone(),
            user: token_user.clone(),
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
        // The token says whose machine it is; every project here is that
        // user's for `[share] mode = "private"`.
        entry.owner_user = token_user.clone();
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
        if !git_sha.is_empty() {
            entry.git_sha = git_sha;
        }
        if !built_at.is_empty() {
            entry.built_at = built_at;
        }
        // The latest word on a project's face wins (a kernel's over an
        // older worker's, a re-registration over the last).
        for (name, face) in identities {
            entry.identities.insert(name, face);
        }
        for (name, mode) in shares {
            entry.shares.insert(name, mode);
        }
        for (name, kind) in kinds {
            entry.kinds.insert(name, kind);
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
    log!(
        "registered {} {} project={} from {peer}",
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
                    // A kernel's notification, with no client attached or
                    // not: pushed to the phones registered for its project
                    // (`<machine>/<project>`); the machine token's user
                    // covers `*` registrations.
                    HubFrame::Notify { project: p, id, agent, kind, title, body, unseen, .. } => {
                        if hub.push.enabled() {
                            let address = format!("{machine}/{p}");
                            let notice = crate::push::Notice { id, agent, kind, title, body, unseen };
                            let hub2 = Arc::clone(&hub);
                            let user = token_user.clone();
                            tokio::spawn(async move {
                                for d in hub2.push.notify(&address, &user, &notice).await {
                                    if d.status != 200 {
                                        log!("push {address} → {}…: {} {}", &d.token[..d.token.len().min(8)], d.status, d.detail.trim());
                                    }
                                }
                            });
                        }
                    }
                    HubFrame::Seen { project: p, unseen, .. } => {
                        if hub.push.enabled() {
                            let address = format!("{machine}/{p}");
                            let hub2 = Arc::clone(&hub);
                            let user = token_user.clone();
                            tokio::spawn(async move {
                                for d in hub2.push.seen(&address, &user, unseen).await {
                                    if d.status != 200 {
                                        log!("badge {address} → {}…: {} {}", &d.token[..d.token.len().min(8)], d.status, d.detail.trim());
                                    }
                                }
                            });
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
                        entry.worktrees.remove(&key);
                    }
                }
            }
            if entry.is_empty() {
                g.machines.remove(&machine);
            }
        }
        g.generation += 1;
    }
    log!(
        "unregistered {} {} project={}",
        kind.as_str(),
        machine,
        project.as_deref().unwrap_or("-")
    );
    hub.broadcast_roster();
}

// ── /attach ─────────────────────────────────────────────────────────────

pub async fn attach(
    hub: Arc<Hub>,
    mut ws: Ws,
    who: Identity,
    machine: &str,
    project: Option<&str>,
) {
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
    // The project's share mode caps the token: a private project of
    // another user is closed to this client; a writer on an open project
    // stays a writer.
    let access = hub.access_of(
        &kernel.machine,
        kernel.project.as_deref().unwrap_or_default(),
        &who,
    );
    if access == "none" {
        let (name, _) = who.as_client();
        log!(
            "{name} refused on {}/{}: the project is not shared with them",
            kernel.machine,
            kernel.project.as_deref().unwrap_or("-")
        );
        let _ = send_json(
            &mut ws,
            &Frame::Error {
                agent: None,
                detail: format!(
                    "no access to {}/{}: the project is not shared with you",
                    kernel.machine,
                    kernel.project.as_deref().unwrap_or("-")
                ),
            },
        )
        .await;
        refuse_close(&mut ws).await;
        return;
    }
    proxy(Arc::clone(&hub), ws, who, kernel, &access).await;
}

/// Join one client socket to one kernel channel until either side ends.
/// `role` is what the client may do there: its token role capped by the
/// project's share mode.
async fn proxy(
    hub_for_push: Arc<Hub>,
    mut ws: Ws,
    who: Identity,
    kernel: Arc<Registrant>,
    role: &str,
) {
    let (to_client, mut from_kernel) = mpsc::unbounded_channel::<serde_json::Value>();
    let (name, _) = who.as_client();
    let role = role.to_string();
    let chan = kernel.open(to_client, &name, &role);
    log!(
        "{name} ({role}) attached to {}/{} chan {chan}",
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
                    // Passed through as the JSON it is: the kernel parses it
                    // on its own version, so a field this hub was built
                    // before (a put's `data`, a history's `before`) arrives
                    // whole. Only the shape is checked here; a line that is
                    // not a frame is refused with the reason, never dropped.
                    match relay_shape(l) {
                        // A phone's device token: the hub's to keep, never
                        // the kernel's. `project` defaults to the address
                        // this socket attached to.
                        Ok(frame) if frame["type"] == "push" => {
                            let address = match frame.get("project").and_then(|v| v.as_str()) {
                                Some(p) if !p.trim().is_empty() => p.trim().to_string(),
                                _ => format!("{}/{}", kernel.machine, kernel.project.as_deref().unwrap_or("")),
                            };
                            let outcome = hub_for_push.push.register(
                                frame["token"].as_str().unwrap_or(""),
                                frame["platform"].as_str().unwrap_or("apns"),
                                frame.get("sandbox").and_then(|v| v.as_bool()),
                                &address,
                                who.user(),
                            );
                            let reply = match outcome {
                                Ok(()) => serde_json::json!({
                                    "type": "pushed",
                                    "project": address,
                                    "enabled": hub_for_push.push.enabled(),
                                    "reason": hub_for_push.push.reason(),
                                }),
                                Err(e) => serde_json::to_value(Frame::Error { agent: None, detail: format!("hub: {e}") }).unwrap_or_default(),
                            };
                            let _ = send_json(&mut ws, &reply).await;
                        }
                        Ok(frame) => {
                            if !kernel.send(HubFrame::Frame { chan, frame }) {
                                break;
                            }
                        }
                        Err(why) => {
                            let _ = send_json(&mut ws, &Frame::Error { agent: None, detail: format!("hub: not relayed: {why}") }).await;
                        }
                    }
                }
            }
        }
    }
    kernel.close(chan, "client left");
    log!("{name} left {} chan {chan}", kernel.machine);
}

/// A client's line as the JSON the kernel will read: an object with a
/// string `type`. Anything else is named back to the client.
fn relay_shape(line: &str) -> std::result::Result<serde_json::Value, String> {
    let value: serde_json::Value =
        serde_json::from_str(line).map_err(|e| format!("not JSON ({e})"))?;
    match value.get("type") {
        Some(serde_json::Value::String(t)) if !t.is_empty() => Ok(value),
        Some(_) => Err("\"type\" is not a string".into()),
        None if value.is_object() => Err("no \"type\" field".into()),
        None => Err("not a JSON object".into()),
    }
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
        refuse_close(&mut ws).await;
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
        refuse_close(&mut ws).await;
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
    log!(
        "{name} claims {} for {project} (isolate={isolate}) id {id}",
        worker.machine
    );
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
        refuse_close(&mut ws).await;
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
    // A worktree place is not a project of the user's: the roster says
    // whose it is, so a client nests or hides it (M-12).
    if isolate && served != project {
        let mut g = hub.inner.lock().unwrap();
        if let Some(entry) = g.machines.get_mut(machine) {
            entry.worktrees.insert(served.clone(), project.clone());
            g.generation += 1;
        }
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
        refuse_close(&mut ws).await;
        return;
    };
    if !send_json(&mut ws, &answer).await {
        return;
    }
    let access = hub.access_of(&kernel.machine, &served, &who);
    let access = if access == "none" {
        // The claimer started this kernel; it is at least a writer there.
        "writer".to_string()
    } else {
        access
    };
    proxy(Arc::clone(&hub), ws, who, kernel, &access).await;
}

#[cfg(test)]
mod roster_face_tests {
    use super::*;

    /// A phone's list draws each project's face from the roster: a live
    /// kernel's project and a worker's checkout alike, as their
    /// registrants read `project.toml`; a project no one read has none.
    #[test]
    fn the_roster_carries_each_projects_face() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let reg = Arc::new(Registrant {
            id: 1,
            machine: "mac".into(),
            user: "owner".into(),
            project: Some("arbos".into()),
            place: Some("/Users/jacob/Code/arbos".into()),
            to_socket: tx,
            chans: Mutex::new(HashMap::new()),
            next_chan: AtomicU64::new(1),
        });
        let mut entry = MachineEntry::default();
        entry.kernels.insert("arbos".into(), reg);
        entry.worker_projects = vec!["arbos".into(), "notes".into(), "bare".into()];
        entry.identities.insert(
            "arbos".into(),
            arbos_core::project::ProjectIdentity {
                name: Some("Arbos".into()),
                icon: "terminal".into(),
                color: "teal".into(),
            },
        );
        entry.identities.insert(
            "notes".into(),
            arbos_core::project::ProjectIdentity {
                name: None,
                icon: "book".into(),
                color: "#336699".into(),
            },
        );
        let info = entry.info("mac", None, "mesh");
        let by_name = |n: &str| info.projects.iter().find(|p| p.name == n).unwrap().clone();
        let arbos = by_name("arbos");
        assert!(arbos.live);
        assert_eq!(arbos.identity.as_ref().unwrap().icon, "terminal");
        assert_eq!(
            arbos.identity.as_ref().unwrap().name.as_deref(),
            Some("Arbos")
        );
        let notes = by_name("notes");
        assert!(!notes.live);
        assert_eq!(notes.identity.as_ref().unwrap().color, "#336699");
        assert!(by_name("bare").identity.is_none());
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["projects"][0]["identity"]["icon"], "terminal");
        assert!(
            json["projects"][0].get("kind").is_none()
                && json["projects"][0].get("parent").is_none(),
            "a project carries no kind or parent: {json}"
        );
    }

    /// M-12: a worker's worktree place (`demo--c616190-1`, a claim with
    /// `isolate`) registered as a project beside `demo`. The roster now
    /// marks it `kind: worktree` with `parent: demo` — from the claim, or
    /// from the name when the hub restarted in between — and it wears
    /// the parent's face.
    #[test]
    fn a_worktree_place_is_marked_with_its_parent() {
        let reg = |id: u64, project: &str| {
            let (tx, _rx) = mpsc::unbounded_channel();
            Arc::new(Registrant {
                id,
                machine: "arboslife".into(),
                user: "owner".into(),
                project: Some(project.into()),
                place: Some(format!("/home/u/arbos/{project}")),
                to_socket: tx,
                chans: Mutex::new(HashMap::new()),
                next_chan: AtomicU64::new(1),
            })
        };
        let mut entry = MachineEntry::default();
        entry.worker_projects = vec!["demo".into(), "notes".into()];
        entry.kernels.insert("demo".into(), reg(1, "demo"));
        entry
            .kernels
            .insert("demo--c616190-1".into(), reg(2, "demo--c616190-1"));
        entry
            .kernels
            .insert("notes--c7-2".into(), reg(3, "notes--c7-2"));
        entry.kernels.insert("solo--x".into(), reg(4, "solo--x"));
        entry
            .worktrees
            .insert("demo--c616190-1".into(), "demo".into());
        entry.identities.insert(
            "demo".into(),
            arbos_core::project::ProjectIdentity {
                name: Some("Demo".into()),
                icon: "flask".into(),
                color: "teal".into(),
            },
        );
        let info = entry.info("arboslife", None, "mesh");
        let by_name = |n: &str| info.projects.iter().find(|p| p.name == n).unwrap().clone();
        let demo = by_name("demo");
        assert!(demo.kind.is_empty() && demo.parent.is_none());
        let wt = by_name("demo--c616190-1");
        assert_eq!(wt.kind, "worktree");
        assert_eq!(wt.parent.as_deref(), Some("demo"));
        assert_eq!(
            wt.identity.as_ref().unwrap().icon,
            "flask",
            "wears the parent's face"
        );
        // Not in the claim map (hub restarted): the name says whose it is
        // when the head is a project here.
        let notes = by_name("notes--c7-2");
        assert_eq!(notes.kind, "worktree");
        assert_eq!(notes.parent.as_deref(), Some("notes"));
        // A `--` name whose head is no project here is left alone.
        let solo = by_name("solo--x");
        assert!(solo.kind.is_empty() && solo.parent.is_none());
        let json = serde_json::to_value(&wt).unwrap();
        assert_eq!(json["kind"], "worktree");
        assert_eq!(json["parent"], "demo");
    }

    /// Federation (2026-09-15): every project carries its store address,
    /// and the roster each recipient gets says what *it* may do there:
    /// `min(token role, share mode)`. Jacob's machines (user `owner`) own
    /// each other's stores; Alice's writer token gets `writer` on a mesh
    /// project, nothing on a private one, and `reader` at least on an
    /// open one. A worktree inherits its parent project's mode.
    #[test]
    fn the_roster_names_each_store_and_the_viewers_access_to_it() {
        let reg = |id: u64, project: &str| {
            let (tx, _rx) = mpsc::unbounded_channel();
            Arc::new(Registrant {
                id,
                machine: "arboslife".into(),
                user: "owner".into(),
                project: Some(project.into()),
                place: Some(format!("/home/u/arbos/{project}")),
                to_socket: tx,
                chans: Mutex::new(HashMap::new()),
                next_chan: AtomicU64::new(1),
            })
        };
        let mut entry = MachineEntry::default();
        entry.owner_user = "owner".into();
        entry.worker_projects = vec!["demo".into(), "diary".into(), "blog".into()];
        entry.kernels.insert("demo".into(), reg(1, "demo"));
        entry.kernels.insert("demo--c1".into(), reg(2, "demo--c1"));
        entry.shares.insert("diary".into(), "private".into());
        entry.shares.insert("blog".into(), "open".into());
        // No viewer: addresses and modes, no access.
        let plain = entry.info("arboslife", None, "mesh");
        let by = |info: &MachineInfo, n: &str| {
            info.projects.iter().find(|p| p.name == n).unwrap().clone()
        };
        let demo = by(&plain, "demo");
        assert_eq!(demo.store, "arbos://arboslife/demo/");
        assert_eq!(demo.share, "mesh");
        assert!(demo.access.is_empty());
        assert_eq!(by(&plain, "demo--c1").store, "arbos://arboslife/demo--c1/");
        assert_eq!(by(&plain, "diary").share, "private");
        // Jacob's other machine: owner everywhere, private included.
        let mine = entry.info("arboslife", Some(("owner", "owner")), "mesh");
        assert_eq!(by(&mine, "demo").access, "owner");
        assert_eq!(by(&mine, "diary").access, "owner");
        assert_eq!(by(&mine, "blog").access, "owner");
        // Alice, a writer client of another user.
        let alice = entry.info("arboslife", Some(("alice", "writer")), "mesh");
        assert_eq!(by(&alice, "demo").access, "writer");
        assert_eq!(
            by(&alice, "demo--c1").access,
            "writer",
            "a worktree shares its parent's mode"
        );
        assert_eq!(by(&alice, "diary").access, "none");
        assert_eq!(by(&alice, "blog").access, "writer");
        // A private worktree of a private project stays private.
        entry.shares.insert("demo".into(), "private".into());
        let alice = entry.info("arboslife", Some(("alice", "writer")), "mesh");
        assert_eq!(by(&alice, "demo--c1").access, "none");
        // The hub's default for an unset project: mesh with one user on the
        // hub, private once another person's token exists. `demo` above is
        // set private now; `demo--c1` follows it; an unset project follows
        // the default.
        entry.shares.remove("demo");
        let private_default = entry.info("arboslife", Some(("alice", "writer")), "private");
        assert_eq!(by(&private_default, "demo").share, "private");
        assert_eq!(by(&private_default, "demo").access, "none");
        assert_eq!(by(&private_default, "demo--c1").access, "none");
        assert_eq!(
            by(&private_default, "blog").access,
            "writer",
            "an explicit open stays open"
        );
        let owner_default = entry.info("arboslife", Some(("owner", "owner")), "private");
        assert_eq!(by(&owner_default, "demo").access, "owner");
        entry.shares.insert("demo".into(), "private".into());
        // A place that declares itself infrastructure carries `service`; a
        // client keeps it out of the human list without reading its name.
        entry.kernels.insert("feedback".into(), reg(9, "feedback"));
        entry.kinds.insert("feedback".into(), "service".into());
        let svc = entry.info("arboslife", Some(("owner", "owner")), "mesh");
        assert_eq!(by(&svc, "feedback").kind, "service");
        assert_eq!(
            by(&svc, "feedback").access,
            "owner",
            "the owner's rights are untouched"
        );
        assert!(by(&svc, "demo").kind.is_empty());
        assert!(
            svc.describe().contains("feedback (service)"),
            "{}",
            svc.describe()
        );
        let json = serde_json::to_value(&by(&alice, "blog")).unwrap();
        assert_eq!(json["store"], "arbos://arboslife/blog/");
        assert_eq!(json["access"], "writer");
        assert_eq!(json["share"], "open");
    }
}

#[cfg(test)]
mod relay_tests {
    use super::relay_shape;

    /// A hub built before a kernel gained a field must not drop it: the
    /// line goes through as the JSON it is. A line that is not a frame is
    /// refused with a reason, never silently forwarded or dropped.
    #[test]
    fn a_frame_is_relayed_whole_and_junk_is_named() {
        let put =
            r#"{"type":"put","path":"attachments/a.jpg","data":"AAAA","future_field":{"x":1}}"#;
        let v = relay_shape(put).unwrap();
        assert_eq!(v["data"], "AAAA");
        assert_eq!(v["future_field"]["x"], 1);
        assert_eq!(serde_json::to_string(&v).unwrap().len(), put.len());
        let history = r#"{"type":"history","agent":"root","before":255,"limit":3}"#;
        assert_eq!(relay_shape(history).unwrap()["before"], 255);
        assert!(relay_shape("hello").unwrap_err().contains("not JSON"));
        assert!(
            relay_shape("[1,2]")
                .unwrap_err()
                .contains("not a JSON object")
        );
        assert!(
            relay_shape(r#"{"agent":"root"}"#)
                .unwrap_err()
                .contains("no \"type\"")
        );
        assert!(
            relay_shape(r#"{"type":5}"#)
                .unwrap_err()
                .contains("not a string")
        );
    }
}
