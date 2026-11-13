//! The relay's side of `arbos-boxd` (docs/box-scaling.md §4.1 and Phase 2): the `boxd` provider.
//! A box on it runs as an OCI bundle under the host's root daemon; the relay asks the daemon to
//! make it, wake it and put it to sleep, and sleeps it itself after 15 idle minutes (§3.1).
//!
//! Off unless `ARBOS_BOXD_URL` is set: that daemon, on loopback, is [`NODE`]. Phase 4 adds the
//! daemons of other hosts, listed in `ARBOS_BOXD_NODES_FILE` and reached over mutual TLS. A new
//! box goes to the node with the most room ([`place`]), `boxes.node_id` says which, and every
//! later call for that box goes there and nowhere else: a box is never moved between nodes. The
//! in-place move of the Docker `runsc` boxes is an operator's step.
//!
//! Phase 3 is here too: a box asleep for days is moved to the daemon's object store (§3.3) and
//! asked back ahead of its wake (§4.2). That half is off unless the daemon's host status says
//! `cold_tier`, which it does only with an object store of its own.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::os::unix::fs::MetadataExt;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use arbos_core::ids::now_ms;
use arbos_core::wire::BoxState;

use crate::boxes;
use crate::db::Sandbox;
use crate::state::AppState;

/// Error text `boxes::stop_box` returns when agentd refused to sleep (a turn or a job is
/// running). The box keeps running.
pub const BUSY: &str = "box is busy";

/// §3.1: a box sleeps after this long with nothing running and nothing due.
pub(crate) const IDLE_MS: i64 = 15 * 60_000;
/// §3.1 condition 3: a routine due this soon keeps the box up.
const ROUTINE_HORIZON_MS: i64 = 20 * 60_000;
/// §4.2: an asleep box is woken this long before its routine is due, so the run finds it up.
const ROUTINE_HEAD_START_MS: i64 = 60_000;
/// §3.3: a checkpoint goes to the object store once its box has slept this long, or at once
/// while the host has less than [`CKPT_EARLY_FREE_GB`] of disk free.
const CKPT_COLD_MS: i64 = 3 * 24 * 3_600_000;
const CKPT_EARLY_FREE_GB: f64 = 150.0;
/// §3.3: a disk goes once its box has slept this long and the host has less than
/// [`WORK_COLD_FREE_GB`] free.
const WORK_COLD_MS: i64 = 14 * 24 * 3_600_000;
const WORK_COLD_FREE_GB: f64 = 250.0;
/// How often sleeping boxes are looked over for the object store, and how many are sent in one
/// pass: an archive packs, uploads and reads back, so a pass is bounded and the oldest go first.
const TIER_TICK: Duration = Duration::from_secs(300);
const ARCHIVES_PER_TICK: usize = 4;
/// §4.2: a cold box is asked back this long before its routine is due.
const PREFETCH_HEAD_START_MS: i64 = 5 * 60_000;
/// A box's user is asked after at most this often, however many presence frames say so.
const WANTED_EVERY_MS: i64 = 60_000;
/// A box that was wanted this lately is not sent to the object store: it was just asked back.
const WANTED_HOLD_MS: i64 = 30 * 60_000;
/// Error text of a wake that found the box still on its way back from the object store. It is
/// not a failed wake: `boxes::run_wake_jobs` looks again in a few seconds.
pub const FETCHING: &str = "the machine is coming back from cold storage";
/// §4.2: below this much free disk on the daemon's host no new box is made. The spec's own
/// number for these boxes, and not `ARBOS_HOST_MIN_FREE_DISK_GB`, which is the gVisor fleet's.
const MIN_FREE_DISK_GB: f64 = 80.0;
/// §4.2 `max_resident_boxes`: awake and waking boxes on the spec's 503 GB host.
/// `ARBOS_BOXD_MAX_RESIDENT` sets it for another one.
const MAX_RESIDENT: usize = 160;
/// Boxes one node is asked to wake at once; the rest wait their turn at that node. The daemon
/// takes no host-wide lock to wake a box, so they do run side by side: eight, at the 275 ms a
/// wake was measured at on Linux, are ~29 a second, a node's whole cap in six seconds. More buys
/// little and costs the boxes: each wake is a `runsc` start on the cores the awake boxes run on,
/// and it is the most wakes whose memory the floor's reading can be missing.
const WAKES_PER_NODE: usize = 8;
/// Resident slots a node keeps free, so a box that wakes on a busy node takes one at once and
/// does not first wait for another box to be put to sleep: with none, at 3,000 and at 5,000
/// users with every node at its cap, half the wakes waited 0.51 s for that sleep before the
/// daemon was asked. It is the wakes a node is asked for at once, so none of those waits; the
/// slots are refilled behind them ([`keep_headroom`]).
///
/// So the cap is the hard ceiling and the cap less this the soft one: below the soft one an
/// idle box keeps its fifteen minutes ([`IDLE_MS`]), and above it the box its user left longest
/// is put to sleep now. That is cheap for its user, whose next message brings it back in the
/// 250 ms a restore was measured at on Linux (275 ms for a clean start). It costs a node at
/// its cap that many of its boxes awake, 8 of 160, and a node with a small cap never gives up
/// more than a quarter.
const HEADROOM: usize = WAKES_PER_NODE;
/// How often a node's headroom is looked at when no wake has asked for it to be.
const HEADROOM_TICK: Duration = Duration::from_secs(5);
/// Error text of a wake on a full host where no awake box could be put to sleep to make room.
pub const FULL: &str = "the host is full and every machine on it is busy";
/// `boxes.node_id` of a box on the daemon of this host, the one `ARBOS_BOXD_URL` names (§4.1
/// "Node 0").
const NODE: &str = "node-0";
/// The /16 a daemon gives its boxes unless it is told another (`ARBOS_BOXD_SUBNET`).
const SUBNET: [u8; 2] = [10, 207];
/// Shorter than this is not a secret anyone generated (the daemon's own rule).
const MIN_TOKEN_LEN: usize = 32;
/// The largest reply read from the daemon. A host status with a thousand boxes is ~150 KB.
const MAX_REPLY: usize = 4 << 20;

/// One daemon: where it is, the bearer it asks for, and what this relay holds it to.
#[derive(Clone)]
struct Node {
    id: String,
    url: String,
    token: String,
    /// This node's own client. Another host's is pinned to that node's CA and carries the
    /// relay's certificate; none follows a redirect or a proxy with the bearer.
    http: reqwest::Client,
    max_resident: usize,
    /// The first two octets of the /16 its boxes' addresses are in. No two nodes share one: the
    /// relay dials a box by that address and sends it the box's MCP token.
    subnet: [u8; 2],
    /// Held while a box is let onto the node, and no longer: counting the node's boxes and
    /// taking a slot for one more are one step, or two wakes at the cap would each take the
    /// same free slot. The daemon is not asked for anything under it.
    door: Arc<tokio::sync::Mutex<()>>,
    slots: Arc<std::sync::Mutex<Slots>>,
    /// Told when a slot or a box on its way out is let go of, and when a slot is taken: who
    /// waits for a slot, and who keeps the headroom.
    freed: Arc<tokio::sync::Notify>,
    taken: Arc<tokio::sync::Notify>,
    /// [`WAKES_PER_NODE`].
    wakes: Arc<tokio::sync::Semaphore>,
}

/// The slots of a node that the relay's box states do not show.
#[derive(Default)]
struct Slots {
    /// Boxes let on and on their way up: each has its slot from the moment it is let on.
    coming: HashSet<String>,
    /// Awake boxes being put to sleep, for a newcomer or for the node's headroom: nobody
    /// else's to put to sleep. Each has its slot still, until its state says it is asleep.
    going: HashSet<String>,
}

/// One box's hold on a slot, or on a box it is putting to sleep, or on both. Dropped when the
/// wake is over: the box is up and its state says so, or it is not and the slot is free again.
struct Slot {
    slots: Arc<std::sync::Mutex<Slots>>,
    freed: Arc<tokio::sync::Notify>,
    user: Option<String>,
    victim: Option<String>,
}

impl Slot {
    fn keep_only_own(&mut self) {
        if let Some(victim) = self.victim.take() {
            self.slots.lock().unwrap().going.remove(&victim);
            self.freed.notify_waiters();
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.keep_only_own();
        if let Some(user) = &self.user {
            self.slots.lock().unwrap().coming.remove(user);
            self.freed.notify_waiters();
        }
    }
}

/// One entry of `ARBOS_BOXD_NODES_FILE`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NodeCfg {
    id: String,
    url: String,
    token_file: String,
    /// The three of mutual TLS, all or none: the CA the node's certificate must chain to, and
    /// the relay's own certificate and key.
    ca_file: Option<String>,
    cert_file: Option<String>,
    key_file: Option<String>,
    max_resident: Option<usize>,
    /// The node's `ARBOS_BOXD_SUBNET`, `10.208.0.0/16`.
    box_subnet: Option<String>,
}

/// The nodes, and what is remembered across them.
#[derive(Clone)]
pub struct Boxd {
    nodes: Vec<Node>,
    /// user_id → when the box was last wanted: a client had one of its chats on screen, or a
    /// routine of its user was five minutes off. In memory only: after a restart a box asked
    /// back a moment ago may be archived again, which sends nothing and costs its next wake
    /// the fetch.
    wanted: Arc<std::sync::Mutex<HashMap<String, i64>>>,
}

/// Every field the daemon sends is named in these, so a reply of any other shape is refused.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ensured {
    created: bool,
    size_gb: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Warm {
    Warm,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct Woken {
    state: Warm,
    restored: bool,
    box_url: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Rest {
    AsleepClean,
    AsleepCkpt,
    /// Asleep with its checkpoint or its disk in the object store.
    Cold,
}

/// What of a sleeping box goes to the object store (the daemon's `Part`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Part {
    Ckpt,
    Work,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Archived {
    archived: bool,
    stored_bytes: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prefetched {
    cold: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Slept {
    state: Rest,
    files_bytes: Option<u64>,
    scratch_bytes: Option<u64>,
    trash_bytes: Option<u64>,
    work_bytes: u64,
    ckpt_bytes: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct HostBox {
    user_id: String,
    running: bool,
    size_gb: u64,
    work_bytes: u64,
    ckpt_bytes: u64,
    /// The two a daemon from before the cold tier does not send.
    #[serde(default)]
    work_cold: bool,
    #[serde(default)]
    ckpt_cold: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct HostStatus {
    mem_available_gb: Option<f64>,
    swap_used_gb: Option<f64>,
    load5: Option<f64>,
    disk_free_gb: f64,
    resident_boxes: u64,
    present_boxes: u64,
    /// The host has an object store. Absent from a daemon that has no cold tier at all.
    #[serde(default)]
    cold_tier: bool,
    boxes: Vec<HostBox>,
}

/// The daemon's own word for a refusal, bounded and scrubbed before it reaches a log or a client.
fn refusal(body: &[u8]) -> String {
    #[derive(Deserialize)]
    struct Refusal {
        error: String,
    }
    serde_json::from_slice::<Refusal>(body)
        .map(|r| arbos_core::redact::scrub(&r.error.chars().take(200).collect::<String>()))
        .unwrap_or_default()
}

/// The path of one box's operation. A user id is one path segment of the daemon's alphabet
/// (`arbos-boxd` `UserId::parse`), or nothing is sent.
fn box_path(user_id: &str, op: &str) -> Result<String> {
    let ok = !user_id.is_empty()
        && user_id.len() <= 64
        && user_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !ok {
        bail!("boxd: {user_id:?} is not a user id the daemon takes");
    }
    Ok(format!("/v1/boxes/{user_id}/{op}"))
}

/// Read a file this relay trusts for what it holds: a plain file, not a link, that nobody but its
/// owner can write, and, when it is a token or a key, that nobody but its owner can read.
fn read_checked(what: &str, path: &str, secret: bool) -> Result<Vec<u8>> {
    let md = std::fs::symlink_metadata(path).with_context(|| format!("boxd {what} {path}"))?;
    let (loose, mode) = if secret {
        (0o077, "with mode 0600")
    } else {
        (0o022, "only its owner can write")
    };
    if !md.is_file() || md.mode() & loose != 0 {
        bail!("boxd {what} {path} must be a plain file {mode}");
    }
    Ok(std::fs::read(path)?)
}

/// `10.208.0.0/16` → `[10, 208]`. A private /16 and nothing else.
fn parse_subnet(s: &str) -> Option<[u8; 2]> {
    let ip: Ipv4Addr = s.strip_suffix("/16")?.parse().ok()?;
    let [a, b, 0, 0] = ip.octets() else {
        return None;
    };
    ip.is_private().then_some([a, b])
}

/// `ARBOS_BOXD_NODES_FILE`: a JSON array of [`NodeCfg`]. It names where the bearer is sent and
/// what is trusted there, so it is a file nobody but its owner can write.
fn nodes_in(file: &str) -> Result<Vec<NodeCfg>> {
    serde_json::from_slice(&read_checked("nodes file", file, false)?)
        .with_context(|| format!("boxd nodes file {file}"))
}

impl Boxd {
    /// `ARBOS_BOXD_URL` and `ARBOS_BOXD_TOKEN_FILE` name the daemon of this host; neither set, the
    /// provider is off. `ARBOS_BOXD_NODES_FILE` lists the daemons of other hosts. Anything named
    /// and not usable stops the relay rather than run a provider that cannot work.
    pub fn from_env() -> Result<Option<Self>> {
        let var = |k: &str| {
            std::env::var(k)
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
        };
        let nodes_file = var("ARBOS_BOXD_NODES_FILE");
        let Some(url) = var("ARBOS_BOXD_URL") else {
            if nodes_file.is_some() {
                bail!("ARBOS_BOXD_NODES_FILE is set; ARBOS_BOXD_URL must name this host's daemon");
            }
            return Ok(None);
        };
        let token_file = var("ARBOS_BOXD_TOKEN_FILE")
            .context("ARBOS_BOXD_URL is set; ARBOS_BOXD_TOKEN_FILE must name the token file")?;
        let max_resident = var("ARBOS_BOXD_MAX_RESIDENT")
            .map(|n| {
                n.parse()
                    .with_context(|| format!("ARBOS_BOXD_MAX_RESIDENT must be a count, not {n}"))
            })
            .transpose()?;
        Self::from_files(&url, &token_file, max_resident, nodes_file.as_deref()).map(Some)
    }

    /// What [`Self::from_env`] read: this host's daemon, and the file of the others if any.
    pub(crate) fn from_files(
        url: &str,
        token_file: &str,
        max_resident: Option<usize>,
        nodes_file: Option<&str>,
    ) -> Result<Self> {
        let mut cfgs = vec![NodeCfg {
            id: NODE.to_owned(),
            url: url.to_owned(),
            token_file: token_file.to_owned(),
            ca_file: None,
            cert_file: None,
            key_file: None,
            max_resident,
            box_subnet: None,
        }];
        if let Some(file) = nodes_file {
            cfgs.extend(nodes_in(file)?);
        }
        Self::new(&cfgs)
    }

    /// No two nodes share an id, a daemon or a box subnet.
    fn new(cfgs: &[NodeCfg]) -> Result<Self> {
        let nodes = cfgs.iter().map(Node::new).collect::<Result<Vec<_>>>()?;
        for (i, n) in nodes.iter().enumerate() {
            let same = |m: &&Node| m.id == n.id || m.url == n.url || m.subnet == n.subnet;
            if let Some(m) = nodes[..i].iter().find(same) {
                bail!(
                    "boxd nodes {} and {} share an id, a url or a box subnet",
                    m.id,
                    n.id
                );
            }
        }
        Ok(Self {
            nodes,
            wanted: Arc::default(),
        })
    }

    fn node(&self, id: &str) -> Result<&Node> {
        self.nodes
            .iter()
            .find(|n| n.id == id)
            .ok_or_else(|| anyhow!("boxd node {id} is not one this relay is configured with"))
    }

    /// Note that the box is wanted now. `false` if it was noted within `every_ms` already.
    fn want(&self, user_id: &str, now: i64, every_ms: i64) -> bool {
        let mut wanted = self.wanted.lock().unwrap_or_else(|e| e.into_inner());
        if wanted.get(user_id).is_some_and(|t| now - *t < every_ms) {
            return false;
        }
        wanted.insert(user_id.to_owned(), now);
        true
    }

    fn wanted_lately(&self, user_id: &str, now: i64) -> bool {
        let wanted = self.wanted.lock().unwrap_or_else(|e| e.into_inner());
        wanted
            .get(user_id)
            .is_some_and(|t| now - *t < WANTED_HOLD_MS)
    }
}

impl Node {
    /// The bearer goes over plain HTTP only to a daemon on loopback. Any other is dialled over
    /// TLS 1.3 that both ends prove: the node's certificate must chain to `ca_file` and to
    /// nothing else this process trusts, and the relay shows `cert_file`. The token and the key
    /// are plain files nobody but their owner can read.
    fn new(cfg: &NodeCfg) -> Result<Self> {
        let id = &cfg.id;
        let named = !id.is_empty()
            && id.len() <= 32
            && id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !named {
            bail!("boxd node id {id:?} must be 1 to 32 of a-z, 0-9 and -");
        }
        let url = cfg.url.trim_end_matches('/');
        let loopback = url
            .strip_prefix("http://")
            .and_then(|a| a.parse::<SocketAddr>().ok())
            .is_some_and(|a| a.ip().is_loopback());
        let mut http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            // A host that is gone is told from one that is slow, in seconds.
            .connect_timeout(Duration::from_secs(5))
            .pool_idle_timeout(Duration::from_secs(300));
        match (loopback, &cfg.ca_file, &cfg.cert_file, &cfg.key_file) {
            (true, None, None, None) => {}
            (true, ..) => {
                bail!("boxd node {id}: ca_file, cert_file and key_file go with an https url")
            }
            (false, Some(ca), Some(cert), Some(key)) => {
                let whole = reqwest::Url::parse(url).is_ok_and(|u| {
                    u.scheme() == "https"
                        && u.has_host()
                        && u.username().is_empty()
                        && u.password().is_none()
                        && u.path() == "/"
                        && u.query().is_none()
                        && u.fragment().is_none()
                });
                if !whole {
                    bail!("boxd node {id}: the url must be https://<host>:<port>, not {url}");
                }
                // What a file fails to parse as is not repeated: one of them is a key.
                let cas =
                    reqwest::Certificate::from_pem_bundle(&read_checked("CA file", ca, false)?)
                        .ok()
                        .filter(|cas| !cas.is_empty())
                        .with_context(|| format!("boxd node {id}: {ca} holds no certificate"))?;
                let mut pem = read_checked("certificate file", cert, false)?;
                pem.push(b'\n');
                pem.extend(read_checked("key file", key, true)?);
                let identity = reqwest::Identity::from_pem(&pem).map_err(|_| {
                    anyhow!("boxd node {id}: {cert} and {key} are not a certificate and its key")
                })?;
                http = http
                    .https_only(true)
                    .tls_built_in_root_certs(false)
                    .min_tls_version(reqwest::tls::Version::TLS_1_3)
                    .identity(identity);
                for ca in cas {
                    http = http.add_root_certificate(ca);
                }
            }
            (false, ..) => bail!(
                "boxd node {id}: the url must be http://127.0.0.1:<port>, or https://<host>:<port> with ca_file, cert_file and key_file, not {url}"
            ),
        }
        let token_file = &cfg.token_file;
        let token = String::from_utf8(read_checked("token file", token_file, true)?)
            .ok()
            .map(|t| t.trim().to_owned())
            .filter(|t| t.len() >= MIN_TOKEN_LEN)
            .with_context(|| {
                format!("boxd token file {token_file} holds fewer than {MIN_TOKEN_LEN} characters")
            })?;
        let subnet = match &cfg.box_subnet {
            None => SUBNET,
            Some(s) => parse_subnet(s).with_context(|| {
                format!(
                    "boxd node {id}: box_subnet must be a private /16 like 10.208.0.0/16, not {s}"
                )
            })?,
        };
        let max_resident = cfg.max_resident.unwrap_or(MAX_RESIDENT);
        if max_resident == 0 {
            bail!("boxd node {id}: max_resident must be a count above zero");
        }
        Ok(Self {
            id: id.clone(),
            url: url.to_owned(),
            token,
            http: http.build().context("boxd node client")?,
            max_resident,
            subnet,
            door: Arc::default(),
            slots: Arc::default(),
            freed: Arc::default(),
            taken: Arc::default(),
            wakes: Arc::new(tokio::sync::Semaphore::new(WAKES_PER_NODE)),
        })
    }

    /// Take a slot for `user`, the box that is to sleep (`victim`), or both.
    fn hold(&self, user: Option<&str>, victim: Option<&str>) -> Slot {
        let mut slots = self.slots.lock().unwrap();
        slots.coming.extend(user.map(str::to_owned));
        slots.going.extend(victim.map(str::to_owned));
        if user.is_some() {
            self.taken.notify_one();
        }
        Slot {
            slots: self.slots.clone(),
            freed: self.freed.clone(),
            user: user.map(str::to_owned),
            victim: victim.map(str::to_owned),
        }
    }

    /// The slots this node keeps free.
    fn headroom(&self) -> usize {
        HEADROOM.min(self.max_resident / 4)
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        timeout: Duration,
    ) -> Result<(StatusCode, Vec<u8>)> {
        let id = &self.id;
        let mut req = self
            .http
            .request(method, format!("{}{path}", self.url))
            .bearer_auth(&self.token)
            .timeout(timeout);
        if let Some(body) = body {
            req = req.json(body);
        }
        // A node that is down, or one whose certificate does not check out: the same words, and
        // the cause behind them for the log.
        let unanswered = || format!("boxd {id} did not answer {path}");
        let mut r = req.send().await.with_context(unanswered)?;
        let status = r.status();
        let mut out = Vec::new();
        while let Some(chunk) = r.chunk().await.with_context(unanswered)? {
            if out.len() + chunk.len() > MAX_REPLY {
                bail!("boxd {id} {path}: a reply over {MAX_REPLY} bytes");
            }
            out.extend_from_slice(&chunk);
        }
        Ok((status, out))
    }

    fn reply<T: DeserializeOwned>(&self, path: &str, status: StatusCode, body: &[u8]) -> Result<T> {
        let id = &self.id;
        if status != StatusCode::OK {
            bail!("boxd {id} {path}: {status} {}", refusal(body));
        }
        serde_json::from_slice(body)
            .map_err(|_| anyhow!("boxd {id} {path}: a reply this relay does not know"))
    }

    /// Make the box's disk and bundle if absent. The first box on a version unpacks its image.
    async fn ensure(
        &self,
        user_id: &str,
        version: &str,
        env: &BTreeMap<String, String>,
    ) -> Result<()> {
        let path = box_path(user_id, "ensure")?;
        let body = json!({ "version": version, "env": env });
        let (status, bytes) = self
            .send(Method::POST, &path, Some(&body), Duration::from_secs(300))
            .await?;
        let made: Ensured = self.reply(&path, status, &bytes)?;
        if ![8, 32, 128].contains(&made.size_gb) {
            bail!(
                "boxd {} {path}: a disk of {} GiB is no tier",
                self.id,
                made.size_gb
            );
        }
        tracing::info!(%user_id, node = %self.id, created = made.created, size_gb = made.size_gb, "boxd: box ensured");
        Ok(())
    }

    /// Start the box, from its checkpoint if it has one. Returns where agentd listens: the
    /// box's own address in this node's subnet, and nothing else is taken for one, because the
    /// relay sends the box's MCP token there.
    async fn wake(&self, user_id: &str) -> Result<String> {
        let path = box_path(user_id, "wake")?;
        let (status, bytes) = self
            .send(Method::POST, &path, None, Duration::from_secs(120))
            .await?;
        let woken: Woken = self.reply(&path, status, &bytes)?;
        let ours = woken
            .box_url
            .strip_prefix("http://")
            .and_then(|a| a.parse::<SocketAddrV4>().ok())
            .is_some_and(|a| a.ip().octets()[..2] == self.subnet && a.port() != 0);
        if !ours {
            // The daemon started it all the same: left running, its agentd would dial in and
            // the relay would call a box awake that it has no address for.
            if let Err(e) = self.sleep(user_id, true).await {
                tracing::warn!(%user_id, node = %self.id, error = %e, "boxd: a refused box was not stopped");
            }
            bail!(
                "boxd {} {path}: the box address is not in this node's subnet",
                self.id
            );
        }
        tracing::info!(%user_id, node = %self.id, restored = woken.restored, "boxd: box awake");
        Ok(woken.box_url)
    }

    /// Stop or checkpoint the box and release its disk. `None` is agentd refusing because the
    /// box is busy: nothing happened to it. With `force` agentd is not asked and nothing that
    /// runs in the box is kept.
    async fn sleep(&self, user_id: &str, force: bool) -> Result<Option<Slept>> {
        let mut path = box_path(user_id, "sleep")?;
        if force {
            path.push_str("?force=true");
        }
        // agentd gets 50 s to stop cleanly, and a checkpoint writes ~0.8 s per GB after that.
        let (status, bytes) = self
            .send(Method::POST, &path, None, Duration::from_secs(180))
            .await?;
        if status == StatusCode::CONFLICT && refusal(&bytes).starts_with("busy") {
            return Ok(None);
        }
        let slept: Slept = self.reply(&path, status, &bytes)?;
        tracing::info!(%user_id, node = %self.id, state = ?slept.state, work_bytes = slept.work_bytes, ckpt_bytes = slept.ckpt_bytes, "boxd: box asleep");
        Ok(Some(slept))
    }

    /// Move a sleeping box's checkpoint or disk to the object store. The daemon answers once
    /// the upload has been read back and the local copy is gone, which takes minutes.
    async fn archive(&self, user_id: &str, part: Part) -> Result<Archived> {
        let path = box_path(user_id, "archive")?;
        let body = json!({ "part": part });
        let (status, bytes) = self
            .send(Method::POST, &path, Some(&body), Duration::from_secs(1800))
            .await?;
        self.reply(&path, status, &bytes)
    }

    /// Ask a cold box back to the host ahead of its wake. `true` while it is still on its way;
    /// an error is the daemon saying why its last fetch failed.
    async fn prefetch(&self, user_id: &str) -> Result<bool> {
        let path = box_path(user_id, "prefetch")?;
        let (status, bytes) = self
            .send(Method::POST, &path, None, Duration::from_secs(30))
            .await?;
        Ok(self.reply::<Prefetched>(&path, status, &bytes)?.cold)
    }

    async fn host(&self) -> Result<HostStatus> {
        let path = "/v1/host/status";
        let (status, bytes) = self
            .send(Method::GET, path, None, Duration::from_secs(30))
            .await?;
        let host: HostStatus = self.reply(path, status, &bytes)?;
        let size = |gb: f64| gb.is_finite() && gb >= 0.0;
        if !size(host.disk_free_gb) || !host.mem_available_gb.is_none_or(size) {
            bail!("boxd {} {path}: free disk or memory is not a size", self.id);
        }
        Ok(host)
    }
}

pub fn of(state: &AppState) -> Result<&Boxd> {
    state
        .cfg
        .boxd
        .as_ref()
        .ok_or_else(|| anyhow!("the boxd provider is off: ARBOS_BOXD_URL is not set"))
}

/// The node a user's box is on: what its row says, or, for a box with no row (one being made,
/// one an admin deleted), where its disk was made. A node this relay no longer lists, or one
/// that is down, is an error for that box; the box is not looked for anywhere else.
fn node_of<'a>(state: &'a AppState, user_id: &str) -> Result<&'a Node> {
    let on_row = state.db.box_lifecycle(user_id)?.map(|(node, ..)| node);
    let id = match on_row {
        Some(id) => id,
        None => state
            .db
            .boxd_node(user_id)?
            .ok_or_else(|| anyhow!("boxd: no node on record for {user_id}"))?,
    };
    of(state)?.node(&id)
}

/// What placement sees of one node that answered.
#[derive(Debug, Clone, Copy)]
pub struct Load {
    /// Boxes it can still wake under its cap; none while it is under its memory floor.
    pub free_slots: usize,
    /// Its cap, and the boxes that live on it, awake or not.
    pub max_resident: usize,
    pub boxes: usize,
    pub disk_free_gb: f64,
}

/// Phase 4's free-headroom placement, pure: which of the nodes a new box goes to. A node that
/// did not answer (`None`) or is under the disk floor takes none. Of the rest, the one with the
/// most free resident slots; then the one with the fewest boxes for its cap, because a box
/// stays where it is made and its node's cap is what its wakes will meet; then the most free
/// disk; then the first listed. A node with no slot left can still be the one: its new box
/// takes an idle one's place ([`take_slot`]).
pub fn place(loads: &[Option<Load>]) -> Option<usize> {
    // Boxes per slot, compared without dividing.
    let crowding = |a: &Load, b: &Load| {
        (a.boxes as u128 * b.max_resident as u128).cmp(&(b.boxes as u128 * a.max_resident as u128))
    };
    loads
        .iter()
        .enumerate()
        .filter_map(|(i, l)| Some((i, l.filter(|l| l.disk_free_gb >= MIN_FREE_DISK_GB)?)))
        .max_by(|(i, a), (j, b)| {
            (a.free_slots.cmp(&b.free_slots))
                .then(crowding(b, a))
                .then(a.disk_free_gb.total_cmp(&b.disk_free_gb))
                .then(j.cmp(i))
        })
        .map(|(i, _)| i)
}

/// The node a box that is being made goes to. One whose disk is already somewhere goes there:
/// the node on record, or the one node that lists it (a disk linked in by the operator's move
/// off Docker, or made a moment before a relay crash). Only a user with no disk is placed.
async fn node_for_new<'a>(state: &'a AppState, user_id: &str) -> Result<&'a Node> {
    let boxd = of(state)?;
    if let Some(id) = state.db.boxd_node(user_id)? {
        return boxd.node(&id);
    }
    let hosts = futures::future::join_all(boxd.nodes.iter().map(Node::host)).await;
    let holds =
        |h: &Result<HostStatus>| matches!(h, Ok(h) if h.boxes.iter().any(|b| b.user_id == user_id));
    let holding: Vec<&Node> = boxd
        .nodes
        .iter()
        .zip(&hosts)
        .filter(|(_, h)| holds(h))
        .map(|(n, _)| n)
        .collect();
    match holding[..] {
        [] => {}
        [node] => return Ok(node),
        _ => bail!("boxd: more than one node holds a disk of {user_id}; none is chosen for them"),
    }
    let residents = residents(state, user_id).await;
    let floor = state.cfg.fleet.evict_min_available_gb;
    let load = |(node, host): (&Node, &Result<HostStatus>)| {
        let h = host.as_ref().ok()?;
        let free = node
            .max_resident
            .saturating_sub(on_node(&residents, &node.id));
        Some(Load {
            free_slots: if h.mem_available_gb.is_none_or(|m| m >= floor) {
                free
            } else {
                0
            },
            max_resident: node.max_resident,
            boxes: h.boxes.len(),
            disk_free_gb: h.disk_free_gb,
        })
    };
    let loads: Vec<Option<Load>> = boxd.nodes.iter().zip(&hosts).map(load).collect();
    if let Some(i) = place(&loads) {
        return Ok(&boxd.nodes[i]);
    }
    // No node takes a new box: say of each why not.
    let why: Vec<String> = boxd
        .nodes
        .iter()
        .zip(hosts)
        .map(|(n, h)| match h {
            Ok(h) => format!("{} has {:.0} GB of disk free", n.id, h.disk_free_gb),
            Err(e) => e.to_string(),
        })
        .collect();
    bail!("no new box below {MIN_FREE_DISK_GB:.0} GB of free disk (fixed for boxd, docs/box-scaling.md §4.2): {}", why.join("; "))
}

/// `arbos-agent-0.6.409` → `0.6.409`, the image version the operator installed on the host. A
/// snapshot is named `arbos-agent-<version>` (the deploy worker's `snapshot="arbos-agent-$version"`),
/// and a version may hold a `-` itself (`0.6.397-pr3005`). Any other name is taken whole.
fn image_version(snapshot: &str) -> &str {
    snapshot.strip_prefix("arbos-agent-").unwrap_or(snapshot)
}

/// Create the user's box on the daemon and start it. The row is written once, when the box is
/// up (`boxes::create_on_provider`, #2262).
pub async fn create_box(
    state: &AppState,
    user_id: &str,
    sb: &mut Sandbox,
    box_token: &str,
    mcp_token: &str,
) -> Result<()> {
    let node = node_for_new(state, user_id).await?;
    let snapshot = sb
        .snapshot
        .clone()
        .unwrap_or_else(|| state.default_snapshot());
    let version = image_version(&snapshot);
    // The id the daemon runs the sandbox under. The dot ends it: runsc takes the start of an
    // id for the whole of it, and no user id holds a dot.
    let name = format!("arbos-box-{user_id}.");
    sb.sandbox_id = Some(name.clone());
    sb.device_id = format!("box:{name}");
    // Kept as for the other providers: the relay's database holds only the box token's hash, and
    // an image roll (a later phase) has to hand the daemon the environment again.
    boxes::save_local_tokens(state, user_id, box_token, mcp_token)?;
    let env = boxes::box_env(state, user_id, box_token, mcp_token, &state.cfg.public_url);
    node.ensure(user_id, version, &env).await?;
    // The disk is on this node from here on, row or no row.
    state.db.set_boxd_node(user_id, &node.id)?;
    sb.snapshot = Some(snapshot);
    start_on(state, node, user_id, sb).await
}

/// Wake an existing box and record where it listens.
pub async fn start_box(state: &AppState, user_id: &str, sb: &mut Sandbox) -> Result<()> {
    start_on(state, node_of(state, user_id)?, user_id, sb).await
}

async fn start_on(state: &AppState, node: &Node, user_id: &str, sb: &mut Sandbox) -> Result<()> {
    // A cold box is fetched before it takes anyone's place on the host or one of the node's
    // wakes, and behind this call: the fetch can take minutes.
    if is_cold(state, user_id) && node.prefetch(user_id).await? {
        bail!(FETCHING);
    }
    let _wake = node.wakes.acquire().await?;
    let _slot = take_slot(state, node, user_id).await?;
    // A box being made has no row yet, and this writes nothing.
    record(state, user_id, &node.id, "waking", None);
    let woken = node.wake(user_id).await;
    if woken.is_err() {
        record(state, user_id, &node.id, "error", None);
    }
    sb.box_url = Some(woken?);
    sb.last_started_at = Some(now_ms());
    state.db.upsert_box(sb)?;
    record(state, user_id, &node.id, "warm", None);
    Ok(())
}

/// Put a box to sleep and keep what the daemon said of it: how it sleeps, and its storage split
/// (§3.3: the Machine card is to be served from this without waking the box). [`BUSY`] when
/// agentd refused and the box keeps running, which `force` (the user's Stop, an operator's
/// delete) does not let it do.
pub async fn stop_box(state: &AppState, user_id: &str, force: bool) -> Result<()> {
    let node = node_of(state, user_id)?;
    let Some(slept) = node.sleep(user_id, force).await? else {
        bail!(BUSY);
    };
    // The split is agentd's: there is none from a box that was not running to be asked.
    let storage = slept.files_bytes.map(|files| {
        json!({ "files_bytes": files, "scratch_bytes": slept.scratch_bytes,
            "trash_bytes": slept.trash_bytes, "work_bytes": slept.work_bytes,
            "ckpt_bytes": slept.ckpt_bytes, "at": now_ms() })
        .to_string()
    });
    let lifecycle = serde_json::to_value(slept.state)?;
    record(
        state,
        user_id,
        &node.id,
        lifecycle.as_str().unwrap_or("error"),
        storage.as_deref(),
    );
    Ok(())
}

/// The row says the box sleeps with its checkpoint or its disk in the object store.
fn is_cold(state: &AppState, user_id: &str) -> bool {
    matches!(state.db.box_lifecycle(user_id), Ok(Some((_, lifecycle, _))) if lifecycle == "cold")
}

/// `boxes.lifecycle_state`: where the daemon last left the box. It is a record of what the
/// daemon said, not what is counted or acted on: a box that died by itself still reads `warm`
/// here until its next wake or sleep, and the relay's own view of it (`AppState::box_state`)
/// is what admission and the idle supervisor go by. The one thing admission reads off it is
/// whether a box told to wake has been started yet ([`see`]).
fn record(
    state: &AppState,
    user_id: &str,
    node_id: &str,
    lifecycle: &str,
    storage_json: Option<&str>,
) {
    if let Err(e) = state
        .db
        .set_box_lifecycle(user_id, node_id, lifecycle, storage_json)
    {
        tracing::warn!(%user_id, lifecycle, error = %e, "boxd: lifecycle state not written");
    }
}

// ---- admission (§4.2) ------------------------------------------------------------------------

/// The `boxd` half of `fleet::Capacity`. `resident` boxes hold memory and are capped; `present`
/// ones hold only disk, and free disk is all that gates a new box. The figures are the sums over
/// the nodes that answered; `nodes` has each one's own.
#[derive(Debug, Clone, Serialize)]
pub struct Capacity {
    pub max_resident: usize,
    /// Awake and waking boxes, as the relay sees them.
    pub resident: usize,
    /// Boxes with a disk on a host, asleep or not, as the daemons count them.
    pub present: u64,
    pub mem_available_gb: Option<f64>,
    pub disk_free_gb: f64,
    /// A new box may be made right now, on some node.
    pub has_room: bool,
    pub nodes: Vec<NodeCapacity>,
}

/// One node's share. A node that did not answer has `up: false` and only what the relay knows.
#[derive(Debug, Clone, Serialize)]
pub struct NodeCapacity {
    pub id: String,
    pub up: bool,
    pub max_resident: usize,
    pub resident: usize,
    pub present: u64,
    pub mem_available_gb: Option<f64>,
    pub disk_free_gb: f64,
    pub has_room: bool,
}

/// `None` when the provider is off, or no daemon gave an answer to count from.
pub async fn capacity(state: &AppState) -> Option<Capacity> {
    let boxd = state.cfg.boxd.as_ref()?;
    let hosts = futures::future::join_all(boxd.nodes.iter().map(Node::host)).await;
    let residents = residents(state, "").await;
    let mut nodes = Vec::new();
    for (node, host) in boxd.nodes.iter().zip(hosts) {
        let host = host
            .inspect_err(|e| tracing::warn!(node = %node.id, error = %e, "boxd: no host status"))
            .ok();
        let free = host.as_ref().map_or(0.0, |h| h.disk_free_gb);
        nodes.push(NodeCapacity {
            id: node.id.clone(),
            up: host.is_some(),
            max_resident: node.max_resident,
            resident: on_node(&residents, &node.id),
            present: host.as_ref().map_or(0, |h| h.present_boxes),
            mem_available_gb: host.as_ref().and_then(|h| h.mem_available_gb),
            disk_free_gb: free,
            has_room: host.is_some() && free >= MIN_FREE_DISK_GB,
        });
    }
    let up = || nodes.iter().filter(|n| n.up);
    up().next()?;
    Some(Capacity {
        max_resident: up().map(|n| n.max_resident).sum(),
        resident: up().fold(0, |sum, n| sum.saturating_add(n.resident)),
        present: up().map(|n| n.present).sum(),
        mem_available_gb: up().filter_map(|n| n.mem_available_gb).reduce(|a, b| a + b),
        disk_free_gb: up().map(|n| n.disk_free_gb).sum(),
        has_room: up().any(|n| n.has_room),
        nodes,
    })
}

/// Awake and waking `boxd` boxes by node, not counting `except` (the one about to wake). `None`
/// when they cannot be counted, which [`on_node`] reads as no room: fail closed.
async fn residents(state: &AppState, except: &str) -> Option<HashMap<String, usize>> {
    let mut by_node = HashMap::new();
    for (user_id, node_id) in state.db.boxd_box_nodes().ok()? {
        if user_id != except
            && matches!(
                state.box_state(&user_id).await.0,
                BoxState::Awake | BoxState::Waking
            )
        {
            *by_node.entry(node_id).or_default() += 1;
        }
    }
    Some(by_node)
}

fn on_node(residents: &Option<HashMap<String, usize>>, node_id: &str) -> usize {
    match residents {
        Some(by_node) => by_node.get(node_id).copied().unwrap_or(0),
        None => usize::MAX,
    }
}

/// The resident gate, pure: under the cap, and the host above its memory floor where the daemon
/// can tell (it cannot off Linux).
pub fn room_to_wake(resident: usize, max: usize, mem_gb: Option<f64>, min_mem_gb: f64) -> bool {
    resident < max && mem_gb.is_none_or(|m| m >= min_mem_gb)
}

/// What the door sees of a node.
struct Seen {
    /// The boxes that have a slot on it, but for the one that asks.
    resident: usize,
    /// Told to wake and not started: on their way to a slot, not in one.
    waiting: usize,
    here: HashMap<String, String>,
    coming: HashSet<String>,
    going: HashSet<String>,
}

async fn see(state: &AppState, node: &Node, user_id: &str) -> Result<Seen> {
    // The slots first and the box states after: a box leaves `coming` only once its state
    // says it is up, so read in this order it is in one of the two.
    let (coming, going) = {
        let slots = node.slots.lock().unwrap();
        (slots.coming.clone(), slots.going.clone())
    };
    let here = state.db.boxd_box_nodes()?;
    let mut up: HashSet<&str> = coming.iter().map(String::as_str).collect();
    let mut waiting = 0;
    for (user, on) in &here {
        if *on != node.id {
            continue;
        }
        // A box told to wake is on the host once the daemon has been asked, and it is in
        // `coming` from then. One whose row still says it sleeps is waiting for its slot:
        // counted, a burst's wakers each saw the others in the free slots and put a box to
        // sleep apiece with room to spare. A row that cannot be read counts: fail closed.
        let asleep_on_its_row = || {
            matches!(
                state.db.box_lifecycle(user),
                Ok(Some((_, at, _))) if !matches!(at.as_str(), "warm" | "waking" | "error")
            )
        };
        match state.box_state(user).await.0 {
            BoxState::Awake => drop(up.insert(user)),
            BoxState::Waking if coming.contains(user) || !asleep_on_its_row() => {
                up.insert(user);
            }
            BoxState::Waking if user != user_id => waiting += 1,
            _ => {}
        }
    }
    up.remove(user_id);
    Ok(Seen {
        resident: up.len(),
        waiting,
        here,
        coming,
        going,
    })
}

impl Seen {
    /// The node's awake boxes that may be put to sleep, but for `user_id` and the ones in
    /// `busy`. A box that dialled in before its wake was answered is awake and still coming:
    /// its row is not written yet, and a sleep now would write the old one back over it.
    async fn idle(
        &self,
        state: &AppState,
        node: &Node,
        user_id: &str,
        busy: &HashSet<String>,
    ) -> Vec<crate::fleet::Candidate> {
        let mut idle = crate::fleet::candidates(state, boxes::Provider::Boxd).await;
        idle.retain(|c| {
            c.user_id != user_id
                && self.here.get(&c.user_id) == Some(&node.id)
                && !self.coming.contains(&c.user_id)
                && !self.going.contains(&c.user_id)
                && !busy.contains(&c.user_id)
        });
        idle
    }
}

/// §4.2: a box that wakes on a full host does not wait in a line. It takes one of the slots the
/// node keeps free ([`HEADROOM`]); when there is none, the least-recently-active awake box with
/// no turn running is put to sleep for it first. One out, one in, and on the same node: the cap
/// and the memory floor are each node's own.
///
/// The slot is taken behind the node's door and the daemon is asked outside it. A box counts
/// from the moment it is let on; the box put to sleep for it counts until it is asleep and is
/// no other newcomer's to take. So wakes run side by side and the node is never over its cap.
/// The memory floor is read once, ahead of the door: what the [`WAKES_PER_NODE`] wakes under
/// way will take is not in the reading yet, which the floor's own margin is for.
async fn take_slot(state: &AppState, node: &Node, user_id: &str) -> Result<Slot> {
    let host = node.host().await?;
    // The memory floor is the one the gVisor fleet evicts at (`ARBOS_HOST_EVICT_MIN_AVAILABLE_GB`).
    let floor = state.cfg.fleet.evict_min_available_gb;
    // Boxes agentd would not let go of.
    let mut busy = HashSet::new();
    loop {
        // Listening from before the look: a slot let go of after it is not missed.
        let freed = node.freed.notified();
        tokio::pin!(freed);
        freed.as_mut().enable();
        let door = node.door.lock().await;
        let seen = see(state, node, user_id).await?;
        if room_to_wake(
            seen.resident,
            node.max_resident,
            host.mem_available_gb,
            floor,
        ) {
            return Ok(node.hold(Some(user_id), None));
        }
        let full = seen.resident >= node.max_resident;
        let idle = seen.idle(state, node, user_id, &busy).await;
        let Some(victim) = crate::fleet::pick_eviction(&idle) else {
            // Every box that could go is going already, for the headroom or for another
            // newcomer: one of them asleep may be this box's slot.
            if full && !seen.going.is_empty() {
                drop(door);
                freed.await;
                continue;
            }
            // Owner's call: with nothing to sleep the wake is refused (and retried by the wake
            // job) rather than let past the cap. Under the cap it is the memory floor that
            // refused, which on a host smaller than the spec's is every wake, the first box
            // included: say that.
            if let Some(mem) = host.mem_available_gb.filter(|_| !full) {
                bail!("boxd host has {mem:.0} GB of memory available; no box wakes below {floor:.0} GB (ARBOS_HOST_EVICT_MIN_AVAILABLE_GB)");
            }
            bail!(FULL)
        };
        let mut slot = node.hold(Some(user_id), Some(&victim));
        drop(door);
        let slept = sleep_box(state, &victim, now_ms()).await;
        // Asleep or not, it is not on its way out any more, and its state says which.
        slot.keep_only_own();
        match slept {
            Ok(()) => {
                tracing::info!(user_id = %victim, for_user = %user_id, node = %node.id, "boxd: host full; idle box put to sleep to make room");
                return Ok(slot);
            }
            // agentd knows of a job or a send the relay does not: the slot is given back, and
            // the next one is asked.
            Err(e) if e.to_string() == BUSY => drop(busy.insert(victim)),
            Err(e) => return Err(e),
        }
    }
}

/// Bring a node back to its [`HEADROOM`]: while it has fewer free slots, the boxes [`take_slot`]
/// would put to sleep for a newcomer are put to sleep ahead of one, by the same ranking and the
/// same rules. Never a box with a turn running, one agentd calls busy, or one on its way up;
/// a node whose every awake box is one of those keeps what room it has.
async fn keep_headroom(state: &AppState, node: &Node) {
    let mut busy = HashSet::new();
    loop {
        let door = node.door.lock().await;
        let Ok(seen) = see(state, node, "").await else {
            return;
        };
        // A box on its way out is a slot on its way back.
        let free = node.max_resident.saturating_sub(seen.resident) + seen.going.len();
        if free >= node.headroom() {
            return;
        }
        let mut idle = seen.idle(state, node, "", &busy).await;
        let mut leaving = Vec::new();
        for _ in free..node.headroom() {
            let Some(victim) = crate::fleet::pick_eviction(&idle) else {
                break;
            };
            idle.retain(|c| c.user_id != victim);
            leaving.push((node.hold(None, Some(&victim)), victim));
        }
        drop(door);
        if leaving.is_empty() {
            return;
        }
        let now = now_ms();
        let sleeps = leaving.into_iter().map(|(slot, user_id)| async move {
            let slept = sleep_box(state, &user_id, now).await;
            drop(slot);
            (user_id, slept)
        });
        for (user_id, slept) in futures::future::join_all(sleeps).await {
            match slept {
                Ok(()) => {
                    tracing::info!(%user_id, node = %node.id, "boxd: idle box put to sleep to keep the node's headroom");
                }
                Err(e) => {
                    if e.to_string() != BUSY {
                        tracing::warn!(%user_id, node = %node.id, error = %e, "boxd: headroom sleep failed");
                    }
                    // Not asked again in this pass, whatever the reason.
                    busy.insert(user_id);
                }
            }
        }
    }
}

/// Keep every node's headroom from now on: looked at whenever a slot is taken on the node, and
/// every [`HEADROOM_TICK`] for the boxes that have gone idle since.
pub(crate) fn spawn_headroom(state: &AppState) {
    let Some(boxd) = &state.cfg.boxd else {
        return;
    };
    for node in boxd.nodes.clone() {
        let state = state.clone();
        tokio::spawn(async move {
            loop {
                keep_headroom(&state, &node).await;
                tokio::select! {
                    _ = node.taken.notified() => {}
                    _ = tokio::time::sleep(HEADROOM_TICK) => {}
                }
            }
        });
    }
}

/// Sleep one box, and wake it again if a client pushed a row for it since `since`: that row has
/// no box to take it.
async fn sleep_box(state: &AppState, user_id: &str, since: i64) -> Result<()> {
    boxes::stop_box(state, user_id, false).await?;
    let pushed = state.last_client_push.lock().await.get(user_id).copied();
    if pushed.is_some_and(|t| t >= since) {
        boxes::ensure_awake(state, user_id, "rows pending after box sleep").await;
    }
    Ok(())
}

// ---- the idle supervisor ---------------------------------------------------------------------

/// What one tick sees of one awake box.
#[derive(Debug, Clone, Copy, Default)]
pub struct Sample {
    /// A keepalive said a turn is running, recently enough to believe.
    pub turn_running: bool,
    /// The user has a routine due within [`ROUTINE_HORIZON_MS`].
    pub routine_soon: bool,
    /// When a client last pushed a row for this box, or 0.
    pub last_client_push_ms: i64,
}

/// One supervisor step for one awake box, pure: `(idle since, sleep now)`. The clock starts at
/// the first quiet tick, restarts at anything that is not quiet, and the box sleeps once it has
/// run for [`IDLE_MS`]. §3.1's fourth condition is half here (the client push) and half in the
/// caller: a box with a wake job is `waking`, and only awake boxes are sampled.
pub fn idle_step(idle_since: Option<i64>, s: &Sample, now: i64) -> (Option<i64>, bool) {
    let since = idle_since.unwrap_or(now);
    if s.turn_running || s.routine_soon || s.last_client_push_ms >= since {
        return (None, false);
    }
    (Some(since), now - since >= IDLE_MS)
}

/// Every minute: sleep each awake `boxd` box that has been idle for 15 minutes. This is what
/// stands in for always-on on these boxes (`boxes::spawn_always_on` leaves them alone).
pub async fn spawn_idle_sleeper(state: AppState) {
    if state.cfg.boxd.is_none() {
        return;
    }
    spawn_headroom(&state);
    tokio::spawn(async move {
        // In memory only: a relay restart gives every box a fresh 15 minutes.
        let mut idle_since = HashMap::new();
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            idle_tick(&state, &mut idle_since).await;
        }
    });
}

pub(crate) async fn idle_tick(state: &AppState, idle_since: &mut HashMap<String, i64>) {
    let now = now_ms();
    // Fail closed: when the routines cannot be read, nothing is put to sleep this minute.
    let (Ok(due), Ok(rows)) = (
        state.db.due_routines(now + ROUTINE_HORIZON_MS),
        state.db.list_boxes(),
    ) else {
        return;
    };
    let soon: HashSet<String> = due
        .iter()
        .filter_map(|rt| state.user_of(&rt.project_id))
        .collect();
    let turns = state.turn_running.lock().await.clone();
    let pushes = state.last_client_push.lock().await.clone();
    let mut kept = HashMap::new();
    for b in rows {
        let user_id = b.user_id;
        if b.provider != boxes::Provider::Boxd.as_str()
            || state.box_state(&user_id).await.0 != BoxState::Awake
        {
            continue;
        }
        let sample = Sample {
            turn_running: turns
                .get(&user_id)
                .is_some_and(|t| now - *t < crate::fleet::TURN_FRESH_MS),
            routine_soon: soon.contains(&user_id),
            last_client_push_ms: pushes.get(&user_id).copied().unwrap_or(0),
        };
        let (since, sleep) = idle_step(idle_since.get(&user_id).copied(), &sample, now);
        if !sleep {
            kept.extend(since.map(|t| (user_id, t)));
            continue;
        }
        // agentd has the last word on the first two conditions (a turn, a waiting send, a job):
        // the daemon asks it, and a refusal restarts this box's clock.
        match sleep_box(state, &user_id, now).await {
            Ok(()) => tracing::info!(%user_id, "boxd: idle box put to sleep"),
            Err(e) if e.to_string() == BUSY => {
                tracing::debug!(%user_id, "boxd: idle box is busy after all; not slept");
            }
            Err(e) => tracing::warn!(%user_id, error = %e, "boxd: idle sleep failed"),
        }
    }
    *idle_since = kept;
}

/// The routine lookahead (§4.2), on the routine scheduler's tick: a sleeping `boxd` box whose
/// user has a routine due within [`PREFETCH_HEAD_START_MS`] is asked back from the object store
/// if it is cold, and woken once the routine is within [`ROUTINE_HEAD_START_MS`]. The same
/// routine then holds it up (`Sample::routine_soon`).
pub async fn wake_ahead_of_routines(state: &AppState, now: i64) {
    if state.cfg.boxd.is_none() {
        return;
    }
    let Ok(due) = state.db.due_routines(now + PREFETCH_HEAD_START_MS) else {
        return;
    };
    // user → their soonest routine. The rows come soonest first.
    let mut users: HashMap<String, i64> = HashMap::new();
    for rt in &due {
        if let (Some(user_id), Some(at)) = (state.user_of(&rt.project_id), rt.next_run_at) {
            users.entry(user_id).or_insert(at);
        }
    }
    for (user_id, at) in users {
        let on_boxd = matches!(state.db.user_box(&user_id), Ok(Some(sb)) if sb.provider == boxes::Provider::Boxd.as_str());
        if !on_boxd || state.box_state(&user_id).await.0 != BoxState::Asleep {
            continue;
        }
        if at <= now + ROUTINE_HEAD_START_MS {
            tracing::info!(%user_id, "boxd: waking ahead of a routine");
            boxes::ensure_awake(state, &user_id, "routine lookahead").await;
        } else {
            prefetch(state, &user_id).await;
        }
    }
}

/// A person is typing in a chat of this project (the `typing` flag on `PRESENCE`, §4.2): a `boxd`
/// box asleep behind it is woken now, the second or three before the message is sent. A box its
/// user stopped stays down until they send or start it, as it does for everything but those.
pub async fn wake_for_typing(state: &AppState, project_id: &str) {
    if let Some(user_id) = asleep_behind(state, project_id).await {
        boxes::ensure_awake(state, &user_id, "typing").await;
    }
}

/// A person has a chat of this project on screen (`focused_chat` on `PRESENCE`): a `boxd` box
/// asleep behind it is woken now, so it is up before the first keystroke, while its node is at
/// or below its soft ceiling ([`HEADROOM`]). On a node above it a look is not worth another
/// box's sleep, and the box waits for the typing or the message.
pub async fn wake_for_focus(state: &AppState, project_id: &str) {
    let Some(user_id) = asleep_behind(state, project_id).await else {
        return;
    };
    let Ok(node) = node_of(state, &user_id) else {
        return;
    };
    let spare = match see(state, node, &user_id).await {
        // The boxes already told to wake will each take a slot: they count here.
        Ok(seen) => seen.resident + seen.waiting + node.headroom() <= node.max_resident,
        Err(_) => false,
    };
    if spare {
        boxes::ensure_awake(state, &user_id, "focus").await;
    }
}

/// The user whose `boxd` box is asleep behind this project, and not held down by their Stop.
async fn asleep_behind(state: &AppState, project_id: &str) -> Option<String> {
    state.cfg.boxd.as_ref()?;
    let user_id = state.user_of(project_id)?;
    let on_boxd = matches!(state.db.user_box(&user_id), Ok(Some(sb)) if sb.provider == boxes::Provider::Boxd.as_str());
    // Fail closed: a hold that cannot be read is a hold.
    let asleep = on_boxd
        && state.box_state(&user_id).await.0 == BoxState::Asleep
        && !state.db.box_held(&user_id).unwrap_or(true);
    asleep.then_some(user_id)
}

// ---- the cold tier (§3.3) and the prefetch (§4.2) --------------------------------------------

/// A client has a chat of this project on screen (`focused_chat` on `PRESENCE`): if the box
/// behind it is cold, ask it back now, so its wake finds it on the host. Whether the look also
/// wakes it is [`wake_for_focus`]'s to say.
pub async fn prefetch_for_project(state: &AppState, project_id: &str) {
    if state.cfg.boxd.is_none() {
        return;
    }
    if let Some(user_id) = state.user_of(project_id) {
        prefetch(state, &user_id).await;
    }
}

/// Ask the user's box back from the object store if it is asleep there. Returns at once; the
/// daemon fetches behind its answer. Presence frames come every few seconds, so a user is
/// looked up at most once a minute, and the note taken here also keeps the box from being
/// archived for [`WANTED_HOLD_MS`].
async fn prefetch(state: &AppState, user_id: &str) {
    let Some(boxd) = state.cfg.boxd.as_ref() else {
        return;
    };
    if !boxd.want(user_id, now_ms(), WANTED_EVERY_MS)
        || !is_cold(state, user_id)
        || state.box_state(user_id).await.0 != BoxState::Asleep
    {
        return;
    }
    let Ok(node) = node_of(state, user_id) else {
        return;
    };
    let (node, user_id) = (node.clone(), user_id.to_owned());
    tokio::spawn(async move {
        match node.prefetch(&user_id).await {
            Ok(cold) => tracing::info!(%user_id, cold, "boxd: cold box asked back"),
            Err(e) => tracing::warn!(%user_id, error = %e, "boxd: prefetch failed"),
        }
    });
}

/// What one pass sees of one sleeping box.
#[derive(Debug, Clone, Copy)]
pub struct Sleeper {
    /// How long ago the box was last stopped.
    pub asleep_ms: i64,
    pub ckpt_local: bool,
    pub work_local: bool,
    /// Wanted lately, or a routine of its user is due soon: it stays where it is.
    pub held: bool,
}

/// §3.3, pure: what of a sleeping box goes to the object store now, if anything. The checkpoint
/// first, after three days or at once on a host short of disk; the disk only after fourteen
/// days, and only while the host is short of disk.
pub fn tier_step(b: &Sleeper, disk_free_gb: f64) -> Option<Part> {
    if b.held {
        return None;
    }
    if b.ckpt_local && (b.asleep_ms >= CKPT_COLD_MS || disk_free_gb < CKPT_EARLY_FREE_GB) {
        return Some(Part::Ckpt);
    }
    let work = b.work_local && b.asleep_ms >= WORK_COLD_MS && disk_free_gb < WORK_COLD_FREE_GB;
    work.then_some(Part::Work)
}

/// Every five minutes: move what [`tier_step`] says to the object store, on a host that has one.
pub async fn spawn_cold_tier(state: AppState) {
    if state.cfg.boxd.is_none() {
        return;
    }
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(TIER_TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            tier_tick(&state).await;
        }
    });
}

pub(crate) async fn tier_tick(state: &AppState) {
    let Ok(boxd) = of(state) else {
        return;
    };
    // One node after another: an archive is minutes of that host's disk and network.
    for node in &boxd.nodes {
        tier_node(state, boxd, node).await;
    }
}

async fn tier_node(state: &AppState, boxd: &Boxd, node: &Node) {
    let now = now_ms();
    // Fail closed: a host that cannot be read, or routines that cannot, archive nothing.
    let (Ok(host), Ok(due), Ok(rows), Ok(here)) = (
        node.host().await,
        state.db.due_routines(now + ROUTINE_HORIZON_MS),
        state.db.list_boxes(),
        state.db.boxd_box_nodes(),
    ) else {
        return;
    };
    if !host.cold_tier {
        return;
    }
    let soon: HashSet<String> = due
        .iter()
        .filter_map(|rt| state.user_of(&rt.project_id))
        .collect();
    let on_host: HashMap<&str, &HostBox> =
        host.boxes.iter().map(|b| (b.user_id.as_str(), b)).collect();
    let mut todo = Vec::new();
    for b in rows {
        // Only a box both the relay and the daemon call asleep, with a stop to count from.
        let (Some(on), Some(stopped)) = (on_host.get(b.user_id.as_str()), b.last_stopped_at) else {
            continue;
        };
        if here.get(&b.user_id) != Some(&node.id)
            || on.running
            || state.box_state(&b.user_id).await.0 != BoxState::Asleep
        {
            continue;
        }
        let sleeper = Sleeper {
            asleep_ms: now - stopped,
            // A checkpoint on the host is one that takes up disk there.
            ckpt_local: on.ckpt_bytes > 0,
            work_local: !on.work_cold,
            held: soon.contains(&b.user_id) || boxd.wanted_lately(&b.user_id, now),
        };
        if let Some(part) = tier_step(&sleeper, host.disk_free_gb) {
            todo.push((sleeper.asleep_ms, b.user_id, part));
        }
    }
    todo.sort_by_key(|(asleep_ms, ..)| std::cmp::Reverse(*asleep_ms));
    for (_, user_id, part) in todo.into_iter().take(ARCHIVES_PER_TICK) {
        match node.archive(&user_id, part).await {
            Ok(a) => {
                tracing::info!(%user_id, ?part, archived = a.archived, stored_bytes = a.stored_bytes, "boxd: box archived");
                // A wake that came meanwhile has the row; it fetches the box back itself.
                if state.box_state(&user_id).await.0 == BoxState::Asleep {
                    record(state, &user_id, &node.id, "cold", None);
                }
            }
            Err(e) => tracing::warn!(%user_id, ?part, error = %e, "boxd: archive failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use axum::extract::{Request, State};
    use axum::response::{IntoResponse, Response};
    use axum::Json;

    use crate::boxes::Provider;
    use crate::people::tests_support::person;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    /// A daemon that answers what `arbos-boxd` answers (its README), unless told otherwise.
    #[derive(Default)]
    struct Fake {
        /// Every authenticated request: path (and query, if any) and body.
        calls: Mutex<Vec<(String, Value)>>,
        /// Replies that differ from the daemon's usual one, by the last path segment.
        replies: Mutex<HashMap<&'static str, (u16, Value)>>,
        /// How long a wake or a sleep takes.
        takes: Mutex<Duration>,
        /// The boxes that are up, from a wake being asked to a sleep having answered, and the
        /// most there were at once.
        up: Mutex<HashSet<String>>,
        most_up: AtomicUsize,
        /// The wakes being answered, and the most there were at once.
        waking: AtomicUsize,
        most_waking: AtomicUsize,
    }

    impl Fake {
        fn answer(&self, op: &'static str, status: u16, body: Value) {
            self.replies.lock().unwrap().insert(op, (status, body));
        }
        fn paths(&self) -> Vec<String> {
            let calls = self.calls.lock().unwrap();
            calls.iter().map(|(p, _)| p.clone()).collect()
        }
    }

    fn usual(op: &str) -> (u16, Value) {
        let body = match op {
            "ensure" => json!({ "created": true, "size_gb": 8 }),
            "wake" => {
                json!({ "state": "warm", "restored": false, "box_url": "http://10.207.0.2:7788" })
            }
            "sleep" => json!({ "state": "asleep_clean", "files_bytes": 1, "scratch_bytes": 2,
                "trash_bytes": null, "work_bytes": 4096, "ckpt_bytes": 0 }),
            "status" => json!({ "mem_available_gb": 400.5, "swap_used_gb": null, "load5": 1.0,
                "disk_free_gb": 900.0, "resident_boxes": 1, "present_boxes": 1,
                "boxes": [{ "user_id": "u_a", "running": true, "size_gb": 8,
                    "work_bytes": 4096, "ckpt_bytes": 0 }] }),
            "archive" => json!({ "archived": true, "stored_bytes": 10 }),
            "prefetch" => json!({ "cold": false }),
            _ => return (404, json!({ "error": "no such route" })),
        };
        (200, body)
    }

    async fn answer(State(fake): State<Arc<Fake>>, req: Request) -> Response {
        let bearer = req.headers().get("authorization").cloned();
        if bearer.and_then(|v| v.to_str().ok().map(str::to_owned))
            != Some(format!("Bearer {TOKEN}"))
        {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        let path = req.uri().path().to_owned();
        let req_uri = req.uri().to_string();
        let bytes = axum::body::to_bytes(req.into_body(), 1 << 20)
            .await
            .unwrap();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        fake.calls.lock().unwrap().push((req_uri, body));
        let op = path.rsplit('/').next().unwrap_or_default();
        let told = fake.replies.lock().unwrap().get(op).cloned();
        let (status, body) = told.unwrap_or_else(|| usual(op));
        let user = path.split('/').nth(3).unwrap_or_default().to_owned();
        let takes = *fake.takes.lock().unwrap();
        match op {
            "wake" => {
                let up = {
                    let mut up = fake.up.lock().unwrap();
                    up.insert(user);
                    up.len()
                };
                fake.most_up.fetch_max(up, Ordering::SeqCst);
                let waking = fake.waking.fetch_add(1, Ordering::SeqCst) + 1;
                fake.most_waking.fetch_max(waking, Ordering::SeqCst);
                tokio::time::sleep(takes).await;
                fake.waking.fetch_sub(1, Ordering::SeqCst);
            }
            "sleep" => {
                tokio::time::sleep(takes).await;
                if status == 200 {
                    fake.up.lock().unwrap().remove(&user);
                }
            }
            _ => {}
        }
        (StatusCode::from_u16(status).unwrap(), Json(body)).into_response()
    }

    /// The fake on a loopback port, and the node that is its client.
    async fn served_as(id: &str, token: &str) -> (Arc<Fake>, Node) {
        let fake = Arc::new(Fake::default());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let app = axum::Router::new()
            .fallback(answer)
            .with_state(fake.clone());
        tokio::spawn(async move { axum::serve(l, app).await });
        let node = Node {
            id: id.to_owned(),
            url,
            token: token.to_owned(),
            http: reqwest::Client::new(),
            max_resident: MAX_RESIDENT,
            subnet: SUBNET,
            door: Arc::default(),
            slots: Arc::default(),
            freed: Arc::default(),
            taken: Arc::default(),
            wakes: Arc::new(tokio::sync::Semaphore::new(WAKES_PER_NODE)),
        };
        (fake, node)
    }

    /// What a node is configured from, with no TLS and the defaults.
    fn cfg(id: &str, url: &str, token_file: &str) -> NodeCfg {
        NodeCfg {
            id: id.to_owned(),
            url: url.to_owned(),
            token_file: token_file.to_owned(),
            ca_file: None,
            cert_file: None,
            key_file: None,
            max_resident: None,
            box_subnet: None,
        }
    }

    fn one(url: &str, token_file: &str) -> Result<Node> {
        Node::new(&cfg(NODE, url, token_file))
    }

    async fn served(token: &str) -> (Arc<Fake>, Node) {
        served_as(NODE, token).await
    }

    /// A relay whose `boxd` provider points at a fake daemon, and one user with a project.
    async fn relay() -> (AppState, Arc<Fake>, String) {
        let (fake, node) = served(TOKEN).await;
        let boxd = Boxd {
            nodes: vec![node],
            wanted: Arc::default(),
        };
        let mut state = test_state(AuthMode::Github);
        Arc::get_mut(&mut state.cfg).expect("one holder").boxd = Some(boxd);
        let (user, _) = person(&state, 9700);
        (state, fake, user)
    }

    #[tokio::test]
    async fn the_client_speaks_the_daemons_routes_with_the_bearer() {
        let (fake, boxd) = served(TOKEN).await;
        let env = BTreeMap::from([("A".to_owned(), "b".to_owned())]);
        boxd.ensure("u_a", "1.2.3", &env).await.unwrap();
        assert_eq!(boxd.wake("u_a").await.unwrap(), "http://10.207.0.2:7788");
        assert!(boxd.sleep("u_a", false).await.unwrap().is_some());
        assert_eq!(boxd.host().await.unwrap().disk_free_gb, 900.0);
        let calls = fake.calls.lock().unwrap().clone();
        assert_eq!(
            calls[0],
            (
                "/v1/boxes/u_a/ensure".to_owned(),
                json!({ "version": "1.2.3", "env": { "A": "b" } })
            )
        );
        let paths: Vec<&str> = calls.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            paths[1..],
            [
                "/v1/boxes/u_a/wake",
                "/v1/boxes/u_a/sleep",
                "/v1/host/status"
            ]
        );

        // The wrong token is refused by the daemon, and the refusal says nothing of either token.
        let (_, stranger) = served("ffffffffffffffffffffffffffffffff").await;
        let refused = stranger.wake("u_a").await.unwrap_err().to_string();
        assert!(refused.contains("401"), "{refused}");
        assert!(
            !refused.contains(TOKEN) && !refused.contains("ffff"),
            "{refused}"
        );
    }

    /// A user id that is not one path segment never leaves the relay.
    #[tokio::test]
    async fn a_bad_user_id_is_not_sent() {
        let (fake, boxd) = served(TOKEN).await;
        let long = "a".repeat(65);
        for id in ["", "a/b", "..", "a b", "a%2fb", "a.b", long.as_str()] {
            assert!(boxd.wake(id).await.is_err(), "{id:?}");
            assert!(boxd.sleep(id, false).await.is_err(), "{id:?}");
        }
        assert!(fake.paths().is_empty());
    }

    /// Only the replies the daemon documents are taken: a field nobody defined, a missing one, a
    /// state that is not one, a box address that is not the box's, a size that is not a size.
    #[tokio::test]
    async fn a_reply_of_another_shape_is_refused() {
        let (fake, boxd) = served(TOKEN).await;
        for wake in [
            json!({ "state": "warm", "restored": false, "box_url": "http://10.207.0.2:7788", "x": 1 }),
            json!({ "state": "warm", "restored": false }),
            json!({ "state": "cold", "restored": false, "box_url": "http://10.207.0.2:7788" }),
            json!({ "state": "warm", "restored": "no", "box_url": "http://10.207.0.2:7788" }),
            json!({ "state": "warm", "restored": false, "box_url": "http://1.1.1.1:7788" }),
            json!({ "state": "warm", "restored": false, "box_url": "http://127.0.0.1:7788" }),
            json!({ "state": "warm", "restored": false, "box_url": "https://10.207.0.2:7788" }),
            json!({ "state": "warm", "restored": false, "box_url": "http://10.207.0.2:7788/x" }),
            json!({ "state": "warm", "restored": false, "box_url": "http://evil.example:7788" }),
            json!("warm"),
        ] {
            fake.answer("wake", 200, wake.clone());
            assert!(boxd.wake("u_a").await.is_err(), "{wake}");
        }
        fake.answer("sleep", 200, json!({ "state": "warm" }));
        assert!(boxd.sleep("u_a", false).await.is_err());
        fake.answer("ensure", 200, json!({ "created": true, "size_gb": 9 }));
        let env = BTreeMap::new();
        assert!(boxd.ensure("u_a", "1.0", &env).await.is_err());
        for disk in [json!(-1.0), json!("900"), Value::Null] {
            let mut status = usual("status").1;
            status["disk_free_gb"] = disk.clone();
            fake.answer("status", 200, status);
            assert!(boxd.host().await.is_err(), "{disk}");
        }
        let mut status = usual("status").1;
        status["mem_available_gb"] = json!(-1.0);
        fake.answer("status", 200, status);
        assert!(boxd.host().await.is_err());
    }

    /// A box the daemon started at an address outside its node's subnet is stopped there, and
    /// the relay does not have it awake.
    #[tokio::test]
    async fn a_box_woken_outside_its_nodes_subnet_is_stopped_on_the_node() {
        let (state, fake, user) = relay().await;
        fake.answer(
            "wake",
            200,
            json!({ "state": "warm", "restored": false, "box_url": "http://10.208.0.2:7788" }),
        );
        let e = boxes::create_box(
            &state,
            &mut state.restart.admit_creation(&user).await,
            Provider::Boxd,
            None,
        )
        .await;
        assert!(e.unwrap_err().to_string().contains("not in this node"));
        let paths = fake.paths();
        assert_eq!(
            paths.last().unwrap(),
            &format!("/v1/boxes/{user}/sleep?force=true")
        );
        assert!(state.db.user_box(&user).unwrap().is_none());
        assert_ne!(state.box_state(&user).await.0, BoxState::Awake);
    }

    /// agentd refusing is not an error and not a sleep; any other refusal is an error, with the
    /// daemon's reason cut to a line.
    #[tokio::test]
    async fn busy_is_told_apart_from_every_other_refusal() {
        let (fake, boxd) = served(TOKEN).await;
        fake.answer("sleep", 409, json!({ "error": "busy: a turn is running" }));
        assert!(boxd.sleep("u_a", false).await.unwrap().is_none());
        fake.answer("sleep", 404, json!({ "error": "x".repeat(5000) }));
        let e = boxd.sleep("u_a", false).await.unwrap_err().to_string();
        assert!(e.contains("404") && e.len() < 300, "{e}");
        fake.answer("sleep", 502, json!({ "error": "agentd did not answer" }));
        assert!(boxd.sleep("u_a", false).await.is_err());
        fake.answer("sleep", 409, json!("busy"));
        assert!(boxd.sleep("u_a", false).await.is_err());
    }

    /// The relay does not start with a daemon that is not on loopback, or a token file others
    /// can read, a link, or a short token.
    #[test]
    fn the_daemon_must_be_local_and_the_token_file_private() {
        let dir = std::env::temp_dir().join(format!("arbos-relay-boxd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("token");
        let p = path.to_str().unwrap();
        let write = |text: &str, mode: u32| {
            std::fs::write(&path, text).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        };
        write(&format!("{TOKEN}\n"), 0o600);
        let boxd = one("http://127.0.0.1:7790/", p).unwrap();
        assert_eq!(
            (boxd.url.as_str(), boxd.token.as_str()),
            ("http://127.0.0.1:7790", TOKEN)
        );
        for url in [
            "http://10.0.0.5:7790",
            "https://127.0.0.1:7790",
            "http://localhost:7790",
            "http://127.0.0.1:7790/v1",
            "http://127.0.0.1",
            "http://user@127.0.0.1:7790",
        ] {
            assert!(one(url, p).is_err(), "{url}");
        }
        write(TOKEN, 0o640);
        assert!(
            one("http://127.0.0.1:7790", p).is_err(),
            "group can read it"
        );
        write("short", 0o600);
        assert!(one("http://127.0.0.1:7790", p).is_err());
        write(TOKEN, 0o600);
        let link = dir.join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(one("http://127.0.0.1:7790", link.to_str().unwrap()).is_err());
        assert!(one(
            "http://127.0.0.1:7790",
            dir.join("missing").to_str().unwrap()
        )
        .is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_box_sleeps_after_fifteen_quiet_minutes_and_anything_restarts_the_clock() {
        let quiet = Sample::default();
        let t0 = 1_000_000;
        // The first quiet tick starts the clock; the box sleeps at the tick 15 minutes later.
        assert_eq!(idle_step(None, &quiet, t0), (Some(t0), false));
        assert_eq!(
            idle_step(Some(t0), &quiet, t0 + IDLE_MS - 1),
            (Some(t0), false)
        );
        assert_eq!(idle_step(Some(t0), &quiet, t0 + IDLE_MS), (Some(t0), true));
        // A turn, a routine inside the horizon, or a client row since the clock started: no
        // sleep, and the 15 minutes start over.
        let late = t0 + IDLE_MS;
        for busy in [
            Sample {
                turn_running: true,
                ..quiet
            },
            Sample {
                routine_soon: true,
                ..quiet
            },
            Sample {
                last_client_push_ms: t0,
                ..quiet
            },
            Sample {
                last_client_push_ms: late,
                ..quiet
            },
        ] {
            assert_eq!(idle_step(Some(t0), &busy, late), (None, false), "{busy:?}");
        }
        // A row from before the clock started does not hold the box up.
        let old = Sample {
            last_client_push_ms: t0 - 1,
            ..quiet
        };
        assert_eq!(idle_step(Some(t0), &old, late), (Some(t0), true));
        assert_eq!(idle_step(None, &old, late), (Some(late), false));
    }

    #[test]
    fn the_image_version_is_what_follows_the_snapshot_prefix() {
        assert_eq!(image_version("arbos-agent-0.6.409"), "0.6.409");
        assert_eq!(
            image_version("arbos-agent-0.6.397-pr3005"),
            "0.6.397-pr3005"
        );
        assert_eq!(image_version("0.6.409"), "0.6.409");
        assert_eq!(image_version("other-1.0"), "other-1.0");
    }

    /// Create, stop and start through the same `boxes` calls every provider goes through.
    #[tokio::test]
    async fn a_box_on_the_provider_is_made_slept_and_woken_through_the_daemon() {
        let (state, fake, user) = relay().await;
        let sb = boxes::create_box(
            &state,
            &mut state.restart.admit_creation(&user).await,
            Provider::Boxd,
            Some("arbos-agent-1.2.3"),
        )
        .await
        .unwrap();
        assert_eq!(sb.provider, "boxd");
        let row = state.db.user_box(&user).unwrap().expect("row");
        assert_eq!(row.box_url.as_deref(), Some("http://10.207.0.2:7788"));
        assert_eq!(row.sandbox_id, Some(format!("arbos-box-{user}.")));
        assert_eq!(row.snapshot.as_deref(), Some("arbos-agent-1.2.3"));
        let ensure = fake.calls.lock().unwrap()[1].clone();
        assert_eq!(ensure.0, format!("/v1/boxes/{user}/ensure"));
        assert_eq!(ensure.1["version"], "1.2.3");
        assert_eq!(ensure.1["env"]["ARBOS_USER_ID"], user.as_str());
        assert!(ensure.1["env"]["ARBOS_BOX_TOKEN"].is_string());

        state.set_box_state(&user, BoxState::Awake, None).await;
        fake.answer("sleep", 409, json!({ "error": "busy: a turn is running" }));
        let busy = boxes::stop_box(&state, &user, false).await.unwrap_err();
        assert_eq!(busy.to_string(), BUSY);
        assert_eq!(state.box_state(&user).await.0, BoxState::Awake);
        fake.replies.lock().unwrap().clear();
        boxes::stop_box(&state, &user, false).await.unwrap();
        assert_eq!(state.box_state(&user).await.0, BoxState::Asleep);
        assert!(state
            .db
            .user_box(&user)
            .unwrap()
            .unwrap()
            .last_stopped_at
            .is_some());

        boxes::start_box(&state, &user).await.unwrap();
        let op = |p: &String| p.rsplit('/').next().unwrap().to_owned();
        let ops: Vec<String> = fake.paths().iter().map(op).collect();
        // The host is asked before a create (its disk) and before every wake (its room).
        assert_eq!(
            ops,
            ["status", "ensure", "status", "wake", "sleep", "sleep", "status", "wake"]
        );
    }

    /// The row says where the daemon last left the box, and keeps the storage split of the last
    /// sleep that had one. Boxes of other providers say nothing of the kind.
    #[tokio::test]
    async fn the_row_keeps_the_lifecycle_state_and_the_last_storage_split() {
        let (state, fake, user) = relay().await;
        let at = |state: &AppState| state.db.box_lifecycle(&user).unwrap();
        boxes::create_box(
            &state,
            &mut state.restart.admit_creation(&user).await,
            Provider::Boxd,
            None,
        )
        .await
        .unwrap();
        assert_eq!(at(&state), Some(("node-0".into(), "warm".into(), None)));

        state.set_box_state(&user, BoxState::Awake, None).await;
        fake.answer("sleep", 409, json!({ "error": "busy: a turn is running" }));
        boxes::stop_box(&state, &user, false).await.unwrap_err();
        assert_eq!(at(&state).unwrap().1, "warm", "a busy box did not sleep");

        fake.answer(
            "sleep",
            200,
            json!({ "state": "asleep_ckpt", "files_bytes": 5, "scratch_bytes": 6,
                "trash_bytes": null, "work_bytes": 4096, "ckpt_bytes": 7 }),
        );
        boxes::stop_box(&state, &user, false).await.unwrap();
        let shown = boxes::provider_status(&state, &user).await;
        assert_eq!(shown["lifecycle_state"], "asleep_ckpt");
        assert_eq!(shown["node_id"], "node-0");
        let mut storage = shown["storage"].clone();
        assert!(storage["at"].take().as_i64().unwrap() > 0);
        assert_eq!(
            storage,
            json!({ "files_bytes": 5, "scratch_bytes": 6, "trash_bytes": null,
                "work_bytes": 4096, "ckpt_bytes": 7, "at": null })
        );

        // A wake the daemon refuses, then one it takes, then the sleep of a box that was not
        // running: no agentd to measure a split, so the one before is kept.
        fake.answer("wake", 500, json!({ "error": "runsc restore failed" }));
        boxes::start_box(&state, &user).await.unwrap_err();
        assert_eq!(at(&state).unwrap().1, "error");
        fake.replies.lock().unwrap().remove("wake");
        boxes::start_box(&state, &user).await.unwrap();
        assert_eq!(at(&state).unwrap().1, "warm");
        fake.answer(
            "sleep",
            200,
            json!({ "state": "asleep_clean", "files_bytes": null, "scratch_bytes": null,
                "trash_bytes": null, "work_bytes": 8192, "ckpt_bytes": 0 }),
        );
        boxes::stop_box(&state, &user, false).await.unwrap();
        let (_, lifecycle, kept) = at(&state).unwrap();
        assert_eq!(lifecycle, "asleep_clean");
        let kept: Value = serde_json::from_str(&kept.unwrap()).unwrap();
        assert_eq!(
            (&kept["files_bytes"], &kept["work_bytes"]),
            (&json!(5), &json!(4096))
        );

        // The row a relay writes for any box leaves all of it alone, and a box of another
        // provider shows what it showed before.
        let mut row = state.db.user_box(&user).unwrap().unwrap();
        state.db.upsert_box(&row).unwrap();
        assert_eq!(at(&state).unwrap().1, "asleep_clean");
        row.user_id = "u_gvisor".into();
        row.provider = "gvisor".into();
        state.db.upsert_box(&row).unwrap();
        let other = boxes::provider_status(&state, "u_gvisor").await;
        assert!(other.get("lifecycle_state").is_none() && other.get("node_id").is_none());
    }

    /// The three columns arrive on a database from before them without touching its rows, the
    /// statements that relay ran still run on it afterwards, and opening it again changes nothing.
    #[test]
    fn the_lifecycle_columns_are_added_to_an_old_database_and_an_old_relay_still_reads_it() {
        let dir = std::env::temp_dir().join(format!("arbos-relay-boxd-db-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("relay.db");
        // What `write_box` and `user_box` ran before this change, column for column.
        const OLD_WRITE: &str = "INSERT INTO boxes(user_id, provider, sandbox_id, daytona_state, device_id, box_token_hash, mcp_token, snapshot,
                last_started_at, last_stopped_at, last_seen_at, box_url, preview_token, local_pid, migrated_from)
             VALUES (?1,'gvisor','s',NULL,'box:s','h','m',NULL,1,NULL,NULL,NULL,NULL,NULL,'[]')";
        const OLD_READ: &str = "SELECT user_id, provider, sandbox_id, daytona_state, device_id, box_token_hash, mcp_token, snapshot,
                last_started_at, last_stopped_at, last_seen_at, box_url, preview_token, local_pid, migrated_from FROM boxes";
        drop(crate::db::Db::open(&path).unwrap());
        let old = rusqlite::Connection::open(&path).unwrap();
        for col in ["node_id", "lifecycle_state", "storage_json"] {
            old.execute(&format!("ALTER TABLE boxes DROP COLUMN {col}"), [])
                .unwrap();
        }
        old.execute(OLD_WRITE, ["u_before"]).unwrap();
        drop(old);

        for _ in 0..2 {
            let db = crate::db::Db::open(&path).expect("migrate");
            assert_eq!(db.user_box("u_before").unwrap().unwrap().provider, "gvisor");
            assert_eq!(db.box_lifecycle("u_before").unwrap(), None);
        }
        let db = crate::db::Db::open(&path).unwrap();
        db.set_box_lifecycle("u_before", NODE, "warm", Some("{}"))
            .unwrap();
        db.set_box_lifecycle("u_nobody", NODE, "warm", None)
            .unwrap();
        drop(db);
        let old = rusqlite::Connection::open(&path).unwrap();
        old.execute(OLD_WRITE, ["u_after"]).unwrap();
        let rows = old
            .prepare(OLD_READ)
            .unwrap()
            .query_map([], |_| Ok(()))
            .unwrap()
            .count();
        assert_eq!(rows, 2);
        drop(old);
        let db = crate::db::Db::open(&path).unwrap();
        assert_eq!(db.box_lifecycle("u_after").unwrap(), None);
        assert_eq!(
            db.box_lifecycle("u_before").unwrap(),
            Some((NODE.into(), "warm".into(), Some("{}".into())))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `DELETE /admin/users/{id}/box` is the admin's alone, stops the box for good on the host
    /// (forced: a busy box too, and no checkpoint kept) and only then drops the row. A box the
    /// daemon did not stop keeps its row, or nothing could stop it afterwards.
    #[tokio::test]
    async fn an_admin_delete_stops_the_box_on_the_host_before_the_row_goes() {
        use axum::extract::Path;
        let (state, fake, user) = relay().await;
        let (_, device) = person(&state, 9701);
        boxes::create_box(
            &state,
            &mut state.restart.admit_creation(&user).await,
            Provider::Boxd,
            None,
        )
        .await
        .unwrap();
        state.set_box_state(&user, BoxState::Awake, None).await;
        let delete =
            |who| crate::api::admin_delete_user_box(State(state.clone()), who, Path(user.clone()));
        let before = fake.paths().len();

        let refused = delete(device).await.unwrap_err();
        assert_eq!(refused, (StatusCode::FORBIDDEN, "admin only".to_owned()));
        assert_eq!(
            fake.paths().len(),
            before,
            "nothing was asked of the daemon"
        );

        fake.answer("sleep", 500, json!({ "error": "umount failed" }));
        let failed = delete(crate::auth::Principal::Admin).await.unwrap_err();
        assert_eq!(failed.0, StatusCode::BAD_GATEWAY);
        assert!(state.db.user_box(&user).unwrap().is_some(), "the row stays");

        fake.replies.lock().unwrap().clear();
        delete(crate::auth::Principal::Admin).await.unwrap();
        assert!(state.db.user_box(&user).unwrap().is_none());
        let forced = format!("/v1/boxes/{user}/sleep?force=true");
        assert_eq!(fake.paths()[before..], [forced.clone(), forced]);
        // The idle supervisor and a box making room never force.
        assert!(!fake.paths()[..before].iter().any(|p| p.contains("force")));
    }

    /// The user's Stop is not forced. agentd refusing it (a turn is running) is a 409, and the
    /// box that runs on is neither held down nor left with its secrets paused; a pause the user
    /// set before stays theirs. Any other failure is the 502 it was, with the hold in place.
    #[tokio::test]
    async fn a_stop_refused_as_busy_leaves_the_box_as_it_was() {
        use axum::extract::Query;
        let (state, fake, user) = relay().await;
        boxes::create_box(
            &state,
            &mut state.restart.admit_creation(&user).await,
            Provider::Boxd,
            None,
        )
        .await
        .unwrap();
        state.set_box_state(&user, BoxState::Awake, None).await;
        let device = || crate::auth::Principal::Device {
            id: "d".into(),
            token: String::new(),
            user_id: user.clone(),
        };
        let stop = || {
            let q = Query(crate::api::StopQuery { force: false });
            crate::api::machine_stop(State(state.clone()), device(), q)
        };
        let paused = || state.db.secrets_settings(&user).unwrap();

        fake.answer(
            "sleep",
            409,
            json!({ "error": "busy: p: a turn is running" }),
        );
        let refused = stop().await.unwrap_err();
        assert_eq!(refused.0, StatusCode::CONFLICT);
        assert!(refused.1.contains("busy"), "{}", refused.1);
        assert!(!state.db.box_held(&user).unwrap());
        assert!(!paused().paused, "{:?}", paused());

        crate::secrets::set_paused(&state, &user, true, "user").await;
        stop().await.unwrap_err();
        assert_eq!(
            paused().paused_by.as_deref(),
            Some("user"),
            "{:?}",
            paused()
        );
        crate::secrets::set_paused(&state, &user, false, "user").await;

        fake.answer("sleep", 500, json!({ "error": "umount failed" }));
        assert_eq!(stop().await.unwrap_err().0, StatusCode::BAD_GATEWAY);
        assert!(state.db.box_held(&user).unwrap());
        assert!(paused().paused);

        fake.replies.lock().unwrap().clear();
        stop().await.unwrap();
        assert!(state.db.box_held(&user).unwrap());
        assert_eq!(paused().paused_by.as_deref(), Some("stop_machine"));
    }

    /// A full disk, a daemon that refuses, or no daemon configured at all: no box, and no row
    /// left behind to be woken later.
    #[tokio::test]
    async fn a_box_that_cannot_be_made_leaves_no_row() {
        let (mut state, fake, user) = relay().await;
        let mut full = usual("status").1;
        full["disk_free_gb"] = json!(79.9);
        fake.answer("status", 200, full);
        let e = boxes::create_box(
            &state,
            &mut state.restart.admit_creation(&user).await,
            Provider::Boxd,
            None,
        )
        .await;
        assert!(e.unwrap_err().to_string().contains("disk"));
        assert_eq!(fake.paths(), ["/v1/host/status"]);
        assert!(state.db.user_box(&user).unwrap().is_none());
        // Nothing is waking: left at `waking`, the stale-box detector bounced a box that was
        // never made and then said it could not reach the relay.
        assert_eq!(state.box_state(&user).await.0, BoxState::Asleep);

        // A host under the memory floor (the default is the spec's 64 GB) wakes no box, and
        // says which floor rather than that it is full.
        fake.replies.lock().unwrap().clear();
        let mut small = usual("status").1;
        small["mem_available_gb"] = json!(14.6);
        fake.answer("status", 200, small);
        let e = boxes::create_box(
            &state,
            &mut state.restart.admit_creation(&user).await,
            Provider::Boxd,
            None,
        )
        .await;
        let e = e.unwrap_err().to_string();
        assert!(
            e.contains("15 GB of memory") && e.contains("ARBOS_HOST_EVICT_MIN_AVAILABLE_GB"),
            "{e}"
        );
        assert!(state.db.user_box(&user).unwrap().is_none());

        fake.replies.lock().unwrap().clear();
        fake.answer(
            "ensure",
            409,
            json!({ "error": "version 9 is not installed" }),
        );
        let e = boxes::create_box(
            &state,
            &mut state.restart.admit_creation(&user).await,
            Provider::Boxd,
            None,
        )
        .await;
        assert!(e.unwrap_err().to_string().contains("not installed"));
        assert!(state.db.user_box(&user).unwrap().is_none());

        Arc::get_mut(&mut state.cfg).expect("one holder").boxd = None;
        let e = boxes::create_box(
            &state,
            &mut state.restart.admit_creation(&user).await,
            Provider::Boxd,
            None,
        )
        .await;
        assert!(e.unwrap_err().to_string().contains("ARBOS_BOXD_URL"));
        assert!(state.db.user_box(&user).unwrap().is_none());
    }

    #[test]
    fn a_wake_has_room_under_the_cap_and_above_the_memory_floor() {
        assert!(room_to_wake(159, 160, Some(64.0), 64.0));
        assert!(!room_to_wake(160, 160, Some(400.0), 64.0), "the cap");
        assert!(!room_to_wake(0, 160, Some(63.9), 64.0), "the memory floor");
        assert!(room_to_wake(0, 160, None, 64.0), "no /proc to read");
        assert!(!room_to_wake(usize::MAX, 160, None, 64.0), "uncounted");
    }

    /// A relay with no `boxd` provider counts what it counted before: the gVisor rows, and
    /// nothing new in what `GET /admin/fleet` shows. With the provider on, a `boxd` box is not
    /// one of those rows; it is counted on its own, awake or present.
    #[tokio::test]
    async fn capacity_without_the_provider_is_what_it_was() {
        let (state, _fake, user) = relay().await;
        let off = test_state(AuthMode::Github);
        for st in [&state, &off] {
            let mut row = Sandbox {
                user_id: "u_gvisor".into(),
                provider: "gvisor".into(),
                sandbox_id: None,
                daytona_state: None,
                device_id: "box:g".into(),
                box_token_hash: "h".into(),
                mcp_token: "m".into(),
                snapshot: None,
                last_started_at: None,
                last_stopped_at: None,
                last_seen_at: None,
                box_url: None,
                preview_token: None,
                local_pid: None,
                migrated_from: Vec::new(),
            };
            st.db.upsert_box(&row).unwrap();
            row.device_id = "box:b".into();
            row.user_id = user.clone();
            row.provider = "boxd".into();
            st.db.upsert_box(&row).unwrap();
            st.db
                .set_box_lifecycle(&user, NODE, "waking", None)
                .unwrap();
            st.set_box_state(&user, BoxState::Waking, None).await;
        }
        let cap = crate::fleet::capacity(&off).await;
        assert_eq!((cap.resident, cap.max_boxes), (1, 80));
        assert!(cap.boxd.is_none());
        let shown = serde_json::to_value(&cap).unwrap();
        let mut keys: Vec<&str> = shown.as_object().unwrap().keys().map(|k| &**k).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "disk_free_gb",
                "has_room",
                "max_boxes",
                "min_free_disk_gb",
                "resident",
                "waitlist"
            ]
        );

        let cap = crate::fleet::capacity(&state).await;
        assert_eq!((cap.resident, cap.max_boxes), (1, 80));
        let boxd = cap.boxd.expect("the provider is on");
        assert_eq!(
            (boxd.resident, boxd.present, boxd.max_resident),
            (1, 1, 160)
        );
        assert!(boxd.has_room);
        state.set_box_state(&user, BoxState::Asleep, None).await;
        assert_eq!(capacity(&state).await.unwrap().resident, 0);
    }

    /// At the cap a waking box takes the place of the least recently active awake one; a box
    /// mid-turn is never the one, and with nothing to sleep the wake is refused.
    #[tokio::test]
    async fn a_wake_on_a_full_host_sleeps_the_least_recently_active_idle_box() {
        let (mut state, fake, a) = relay().await;
        let cfg = Arc::get_mut(&mut state.cfg).expect("one holder");
        cfg.boxd.as_mut().unwrap().nodes[0].max_resident = 2;
        let (b, _) = person(&state, 9701);
        let (c, _) = person(&state, 9702);
        let sleeps = |user: &str| {
            let path = format!("/v1/boxes/{user}/sleep");
            fake.paths().iter().filter(|p| **p == path).count()
        };
        for user in [&a, &b] {
            boxes::create_box(
                &state,
                &mut state.restart.admit_creation(user).await,
                Provider::Boxd,
                None,
            )
            .await
            .unwrap();
            state.set_box_state(user, BoxState::Awake, None).await;
        }
        assert_eq!(
            sleeps(&a) + sleeps(&b),
            0,
            "two boxes fit under a cap of two"
        );
        // `a` was used last, so `b` is the one that goes.
        let mut pushes = state.last_client_push.lock().await;
        pushes.insert(a.clone(), now_ms() + 60_000);
        drop(pushes);
        boxes::create_box(
            &state,
            &mut state.restart.admit_creation(&c).await,
            Provider::Boxd,
            None,
        )
        .await
        .unwrap();
        assert_eq!((sleeps(&a), sleeps(&b)), (0, 1));
        assert_eq!(state.box_state(&b).await.0, BoxState::Asleep);
        assert_eq!(state.box_state(&a).await.0, BoxState::Awake);
        state.set_box_state(&c, BoxState::Awake, None).await;

        // `a` is the least recently active now only if its turn is not counted: it is running
        // one, so `c` goes instead.
        state.last_client_push.lock().await.clear();
        let mut row = state.db.user_box(&a).unwrap().unwrap();
        row.last_started_at = Some(1);
        row.last_seen_at = None;
        state.db.upsert_box(&row).unwrap();
        state.turn_running.lock().await.insert(a.clone(), now_ms());
        boxes::start_box(&state, &b).await.unwrap();
        assert_eq!((sleeps(&a), sleeps(&c)), (0, 1));
        state.set_box_state(&b, BoxState::Awake, None).await;

        // Both awake boxes are busy, one by the relay's account and one by agentd's: `c` stays
        // asleep, and the daemon is not asked to wake it.
        fake.answer("sleep", 409, json!({ "error": "busy: a job is running" }));
        let wakes = fake.paths().len();
        let full = boxes::start_box(&state, &c).await.unwrap_err();
        assert_eq!(full.to_string(), FULL);
        assert_eq!(sleeps(&b), 2, "agentd was asked, and said busy");
        assert!(!fake.paths()[wakes..].iter().any(|p| p.ends_with("/wake")));
        assert_eq!(state.box_state(&b).await.0, BoxState::Awake);

        // A daemon that cannot say how the host is doing wakes nothing.
        fake.answer("status", 500, json!({ "error": "runsc list failed" }));
        assert!(boxes::start_box(&state, &c).await.is_err());
        assert!(!fake.paths()[wakes..].iter().any(|p| p.ends_with("/wake")));
    }

    /// What ranks a box for the sleep at the cap is its user's last turn, kept after the turn
    /// ends, and never the stamp the box's own requests leave on its row.
    #[tokio::test]
    async fn a_box_is_ranked_by_its_last_turn_and_not_by_its_own_requests() {
        let (state, _fake, user) = relay().await;
        boxes::create_box(
            &state,
            &mut state.restart.admit_creation(&user).await,
            Provider::Boxd,
            None,
        )
        .await
        .unwrap();
        let mut row = state.db.user_box(&user).unwrap().unwrap();
        row.last_started_at = Some(1);
        state.db.upsert_box(&row).unwrap();
        let ranked = |state: &AppState| {
            let state = state.clone();
            async move { crate::fleet::candidates(&state, Provider::Boxd).await[0].last_active_ms }
        };
        // A keepalive with no turn is the box talking: it stamps the row and ranks nothing.
        boxes::keepalive(&state, &user, false).await.unwrap();
        let seen = state.db.user_box(&user).unwrap().unwrap().last_seen_at;
        assert!(seen.is_some_and(|t| t > 1), "{seen:?}");
        assert_eq!(ranked(&state).await, 1);

        boxes::keepalive(&state, &user, true).await.unwrap();
        boxes::keepalive(&state, &user, false).await.unwrap();
        assert!(state.turn_running.lock().await.is_empty());
        let turn = state.last_turn.lock().await[&user];
        assert_eq!(ranked(&state).await, turn);
    }

    /// A routine due within the minute wakes its user's sleeping box; one further off, a box
    /// that is not asleep, a box on another provider or a relay without the provider does not.
    #[tokio::test]
    async fn an_asleep_box_is_woken_a_minute_ahead_of_its_routine() {
        let (mut state, _fake, user) = relay().await;
        let project_id = crate::people::tests_support::project_of(&state, &user, "First");
        boxes::create_box(
            &state,
            &mut state.restart.admit_creation(&user).await,
            Provider::Boxd,
            None,
        )
        .await
        .unwrap();
        let now = now_ms();
        let due = |id: &str, at: i64| crate::db::Routine {
            id: id.into(),
            project_id: project_id.clone(),
            chat_id: "c".into(),
            name: "n".into(),
            prompt: "p".into(),
            trigger: json!({ "kind": "cron", "expr": "0 9 * * *", "tz": "UTC" }),
            enabled: true,
            created_at: now,
            last_run_at: None,
            next_run_at: Some(at),
        };
        let waking = |state: &AppState| {
            let (state, user) = (state.clone(), user.clone());
            async move {
                let job = state.db.due_wake_jobs().unwrap();
                let st = state.box_state(&user).await.0;
                let _ = state.db.finish_wake_job(&user);
                (st, job.len())
            }
        };

        state
            .db
            .insert_routine(&due("far", now + ROUTINE_HEAD_START_MS + 1))
            .unwrap();
        state.set_box_state(&user, BoxState::Asleep, None).await;
        wake_ahead_of_routines(&state, now).await;
        assert_eq!(waking(&state).await, (BoxState::Asleep, 0), "too early");

        state
            .db
            .insert_routine(&due("near", now + ROUTINE_HEAD_START_MS))
            .unwrap();
        for st in [BoxState::Awake, BoxState::Error] {
            state.set_box_state(&user, st, None).await;
            wake_ahead_of_routines(&state, now).await;
            assert_eq!(waking(&state).await, (st, 0));
        }
        state.set_box_state(&user, BoxState::Asleep, None).await;
        wake_ahead_of_routines(&state, now).await;
        assert_eq!(waking(&state).await, (BoxState::Waking, 1));

        // The same routine and the same sleeping box, on a relay that has no `boxd` provider
        // and on a box of another provider.
        state.set_box_state(&user, BoxState::Asleep, None).await;
        let mut row = state.db.user_box(&user).unwrap().unwrap();
        row.provider = "gvisor".into();
        state.db.upsert_box(&row).unwrap();
        wake_ahead_of_routines(&state, now).await;
        assert_eq!(waking(&state).await, (BoxState::Asleep, 0));
        row.provider = "boxd".into();
        state.db.upsert_box(&row).unwrap();
        Arc::get_mut(&mut state.cfg).expect("one holder").boxd = None;
        wake_ahead_of_routines(&state, now).await;
        assert_eq!(waking(&state).await, (BoxState::Asleep, 0));
    }

    /// One supervisor tick: the idle box sleeps, the one mid-turn does not and loses its clock,
    /// and a box agentd calls busy stays awake.
    #[tokio::test]
    async fn the_supervisor_sleeps_only_the_idle_box() {
        let (state, fake, user) = relay().await;
        boxes::create_box(
            &state,
            &mut state.restart.admit_creation(&user).await,
            Provider::Boxd,
            None,
        )
        .await
        .unwrap();
        state.set_box_state(&user, BoxState::Awake, None).await;
        let long_ago = now_ms() - IDLE_MS - 1;
        let clock = || HashMap::from([(user.clone(), long_ago)]);
        let sleeps = || {
            fake.paths()
                .iter()
                .filter(|p| p.ends_with("/sleep"))
                .count()
        };

        let mut idle_since = HashMap::new();
        idle_tick(&state, &mut idle_since).await;
        assert!(idle_since.contains_key(&user), "the clock starts");
        assert_eq!(sleeps(), 0);

        state
            .turn_running
            .lock()
            .await
            .insert(user.clone(), now_ms());
        let mut idle_since = clock();
        idle_tick(&state, &mut idle_since).await;
        assert!(idle_since.is_empty());
        assert_eq!(sleeps(), 0);
        state.turn_running.lock().await.clear();

        fake.answer("sleep", 409, json!({ "error": "busy: a job is running" }));
        let mut idle_since = clock();
        idle_tick(&state, &mut idle_since).await;
        assert!(idle_since.is_empty());
        assert_eq!(state.box_state(&user).await.0, BoxState::Awake);
        fake.replies.lock().unwrap().clear();

        let mut idle_since = clock();
        idle_tick(&state, &mut idle_since).await;
        assert_eq!(sleeps(), 2);
        assert_eq!(state.box_state(&user).await.0, BoxState::Asleep);
        // Asleep boxes are not sampled, so nothing asks the daemon again.
        idle_tick(&state, &mut clock()).await;
        assert_eq!(sleeps(), 2);
    }

    // ---- the cold tier and the prefetch ----

    const DAY: i64 = 24 * 3_600_000;

    /// What the daemon says of a host with an object store and one sleeping box on it.
    fn cold_host(user: &str, disk_free_gb: f64, ckpt_bytes: u64, work_cold: bool) -> Value {
        json!({ "mem_available_gb": 400.0, "swap_used_gb": 0.0, "load5": 1.0,
            "disk_free_gb": disk_free_gb, "resident_boxes": 0, "present_boxes": 1,
            "cold_tier": true,
            "boxes": [{ "user_id": user, "running": false, "size_gb": 8, "work_bytes": 4096,
                "ckpt_bytes": ckpt_bytes, "work_cold": work_cold, "ckpt_cold": false }] })
    }

    /// A box made on the fake daemon and asleep since `asleep_ms` ago.
    async fn asleep_for(state: &AppState, user: &str, asleep_ms: i64) {
        boxes::create_box(
            state,
            &mut state.restart.admit_creation(user).await,
            Provider::Boxd,
            None,
        )
        .await
        .unwrap();
        boxes::stop_box(state, user, false).await.unwrap();
        let mut row = state.db.user_box(user).unwrap().unwrap();
        row.last_stopped_at = Some(now_ms() - asleep_ms);
        state.db.upsert_box(&row).unwrap();
    }

    fn calls(fake: &Fake, op: &str) -> Vec<(String, Value)> {
        let calls = fake.calls.lock().unwrap();
        let of_op = |c: &&(String, Value)| c.0.ends_with(op);
        calls.iter().filter(of_op).cloned().collect()
    }

    async fn eventually(what: &str, done: impl Fn() -> bool) {
        for _ in 0..500 {
            if done() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("never: {what}");
    }

    /// §3.3, to the millisecond and the gigabyte: a checkpoint at three days or under 150 GB
    /// free, a disk at fourteen days and under 250 GB free, the checkpoint first, and nothing
    /// of a box that is wanted or has nothing on the host.
    #[test]
    fn a_checkpoint_goes_cold_at_three_days_and_a_disk_at_fourteen_on_a_short_host() {
        let b = |asleep_ms, ckpt_local, work_local| Sleeper {
            asleep_ms,
            ckpt_local,
            work_local,
            held: false,
        };
        let roomy = 900.0;
        assert_eq!(tier_step(&b(3 * DAY - 1, true, true), roomy), None);
        assert_eq!(tier_step(&b(3 * DAY, true, true), roomy), Some(Part::Ckpt));
        assert_eq!(tier_step(&b(60_000, true, true), 150.0), None);
        assert_eq!(tier_step(&b(60_000, true, true), 149.9), Some(Part::Ckpt));
        assert_eq!(
            tier_step(&b(60_000, false, true), 10.0),
            None,
            "a young disk stays"
        );

        assert_eq!(tier_step(&b(14 * DAY, false, true), roomy), None);
        assert_eq!(tier_step(&b(14 * DAY, false, true), 250.0), None);
        assert_eq!(tier_step(&b(14 * DAY - 1, false, true), 249.9), None);
        assert_eq!(
            tier_step(&b(14 * DAY, false, true), 249.9),
            Some(Part::Work)
        );
        assert_eq!(tier_step(&b(14 * DAY, true, true), 249.9), Some(Part::Ckpt));
        assert_eq!(tier_step(&b(99 * DAY, false, false), 1.0), None);
        let held = Sleeper {
            held: true,
            ..b(99 * DAY, true, true)
        };
        assert_eq!(tier_step(&held, 1.0), None);
    }

    /// One pass: nothing on a host without an object store; then the old checkpoint goes and
    /// the row says cold. Not a box that is awake, one a client just looked at, one with a
    /// routine near, or one whose archive the daemon refused.
    #[tokio::test]
    async fn a_pass_archives_what_is_due_and_only_on_a_host_with_a_store() {
        let (state, fake, user) = relay().await;
        asleep_for(&state, &user, 4 * DAY).await;
        let lifecycle = || state.db.box_lifecycle(&user).unwrap().unwrap().1;
        let mut host = cold_host(&user, 900.0, 4096, false);
        host["cold_tier"] = json!(false);
        fake.answer("status", 200, host.clone());
        tier_tick(&state).await;
        // A daemon from before the cold tier says nothing of one.
        host.as_object_mut().unwrap().remove("cold_tier");
        fake.answer("status", 200, host);
        tier_tick(&state).await;
        assert!(calls(&fake, "/archive").is_empty());

        fake.answer("status", 200, cold_host(&user, 900.0, 4096, false));
        let boxd = state.cfg.boxd.as_ref().unwrap();
        assert!(boxd.want(&user, now_ms(), WANTED_EVERY_MS));
        tier_tick(&state).await;
        assert!(calls(&fake, "/archive").is_empty(), "wanted a moment ago");
        boxd.wanted.lock().unwrap().clear();

        state.set_box_state(&user, BoxState::Awake, None).await;
        tier_tick(&state).await;
        state.set_box_state(&user, BoxState::Asleep, None).await;
        fake.answer(
            "archive",
            502,
            json!({ "error": "archive failed: object store PUT boxes/u/ckpt/00000: 503" }),
        );
        tier_tick(&state).await;
        assert_eq!(
            calls(&fake, "/archive").len(),
            1,
            "an awake box is not asked for"
        );
        assert_eq!(
            lifecycle(),
            "asleep_clean",
            "a failed archive changes nothing"
        );

        fake.replies.lock().unwrap().remove("archive");
        tier_tick(&state).await;
        let sent = calls(&fake, "/archive");
        assert_eq!(
            sent[1],
            (
                format!("/v1/boxes/{user}/archive"),
                json!({ "part": "ckpt" })
            )
        );
        assert_eq!(lifecycle(), "cold");

        // Its disk, ten days on, on a host short of disk. A routine twenty minutes off holds it.
        let mut row = state.db.user_box(&user).unwrap().unwrap();
        row.last_stopped_at = Some(now_ms() - 14 * DAY);
        state.db.upsert_box(&row).unwrap();
        fake.answer("status", 200, cold_host(&user, 200.0, 0, false));
        let project_id = crate::people::tests_support::project_of(&state, &user, "First");
        let routine = crate::db::Routine {
            id: "soon".into(),
            project_id,
            chat_id: "c".into(),
            name: "n".into(),
            prompt: "p".into(),
            trigger: json!({ "kind": "cron", "expr": "0 9 * * *", "tz": "UTC" }),
            enabled: true,
            created_at: now_ms(),
            last_run_at: None,
            next_run_at: Some(now_ms() + ROUTINE_HORIZON_MS - 60_000),
        };
        state.db.insert_routine(&routine).unwrap();
        tier_tick(&state).await;
        assert_eq!(calls(&fake, "/archive").len(), 2, "a routine is near");
        state.db.delete_routine("soon").unwrap();
        tier_tick(&state).await;
        assert_eq!(calls(&fake, "/archive")[2].1, json!({ "part": "work" }));
        // All of it is in the store now: there is nothing left to send.
        fake.answer("status", 200, cold_host(&user, 200.0, 0, true));
        tier_tick(&state).await;
        assert_eq!(calls(&fake, "/archive").len(), 3);
    }

    /// A pass sends at most four boxes, the longest asleep first.
    #[tokio::test]
    async fn a_pass_is_bounded_and_takes_the_longest_asleep_first() {
        let (state, fake, first) = relay().await;
        let mut users = vec![first];
        for n in 0..5 {
            users.push(person(&state, 9710 + n).0);
        }
        let mut host = cold_host("x", 900.0, 4096, false);
        let mut on_host = Vec::new();
        for (n, user) in users.iter().enumerate() {
            asleep_for(&state, user, (4 + n as i64) * DAY).await;
            on_host.push(json!({ "user_id": user, "running": false, "size_gb": 8,
                "work_bytes": 4096, "ckpt_bytes": 4096 }));
        }
        host["boxes"] = json!(on_host);
        fake.answer("status", 200, host);
        tier_tick(&state).await;
        let sent: Vec<String> = calls(&fake, "/archive").into_iter().map(|c| c.0).collect();
        let oldest: Vec<String> = users
            .iter()
            .rev()
            .take(ARCHIVES_PER_TICK)
            .map(|u| format!("/v1/boxes/{u}/archive"))
            .collect();
        assert_eq!(sent, oldest);
    }

    /// A chat on screen asks its cold box back, once a minute at most, and wakes nothing. A
    /// box that is not cold, or not asleep, is not asked for; nor is anything without the provider.
    #[tokio::test]
    async fn a_focused_chat_asks_a_cold_box_back_and_does_not_wake_it() {
        let (mut state, fake, user) = relay().await;
        let project_id = crate::people::tests_support::project_of(&state, &user, "First");
        asleep_for(&state, &user, DAY).await;
        let wanted = |state: &AppState| state.cfg.boxd.as_ref().unwrap().wanted.clone();
        prefetch_for_project(&state, &project_id).await;
        assert!(
            wanted(&state).lock().unwrap().contains_key(&user),
            "a look is noted"
        );
        wanted(&state).lock().unwrap().clear();

        record(&state, &user, NODE, "cold", None);
        state.set_box_state(&user, BoxState::Awake, None).await;
        prefetch_for_project(&state, &project_id).await;
        wanted(&state).lock().unwrap().clear();
        state.set_box_state(&user, BoxState::Asleep, None).await;
        prefetch_for_project(&state, "no-such-project").await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(calls(&fake, "/prefetch").is_empty());

        prefetch_for_project(&state, &project_id).await;
        prefetch_for_project(&state, &project_id).await;
        eventually("the daemon is asked", || {
            !calls(&fake, "/prefetch").is_empty()
        })
        .await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(calls(&fake, "/prefetch").len(), 1, "once a minute");
        assert!(calls(&fake, "/wake").len() == 1 && state.db.due_wake_jobs().unwrap().is_empty());
        assert_eq!(state.box_state(&user).await.0, BoxState::Asleep);

        wanted(&state).lock().unwrap().clear();
        Arc::get_mut(&mut state.cfg).expect("one holder").boxd = None;
        prefetch_for_project(&state, &project_id).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(calls(&fake, "/prefetch").len(), 1);
    }

    /// Five minutes ahead of a routine a cold box is asked back, not woken; a minute ahead it
    /// is woken.
    #[tokio::test]
    async fn a_cold_box_is_asked_back_five_minutes_ahead_of_its_routine() {
        let (state, fake, user) = relay().await;
        let project_id = crate::people::tests_support::project_of(&state, &user, "First");
        asleep_for(&state, &user, DAY).await;
        record(&state, &user, NODE, "cold", None);
        let now = now_ms();
        let due = |id: &str, at: i64| crate::db::Routine {
            id: id.into(),
            project_id: project_id.clone(),
            chat_id: "c".into(),
            name: "n".into(),
            prompt: "p".into(),
            trigger: json!({ "kind": "cron", "expr": "0 9 * * *", "tz": "UTC" }),
            enabled: true,
            created_at: now,
            last_run_at: None,
            next_run_at: Some(at),
        };
        state
            .db
            .insert_routine(&due("later", now + PREFETCH_HEAD_START_MS + 1))
            .unwrap();
        wake_ahead_of_routines(&state, now).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(calls(&fake, "/prefetch").is_empty(), "too early");

        state
            .db
            .insert_routine(&due("five", now + PREFETCH_HEAD_START_MS))
            .unwrap();
        wake_ahead_of_routines(&state, now).await;
        eventually("the daemon is asked", || {
            calls(&fake, "/prefetch").len() == 1
        })
        .await;
        assert!(
            state.db.due_wake_jobs().unwrap().is_empty(),
            "asked back, not woken"
        );
        assert_eq!(state.box_state(&user).await.0, BoxState::Asleep);

        state
            .db
            .insert_routine(&due("one", now + ROUTINE_HEAD_START_MS))
            .unwrap();
        wake_ahead_of_routines(&state, now).await;
        assert_eq!(state.box_state(&user).await.0, BoxState::Waking);
        assert_eq!(state.db.due_wake_jobs().unwrap().len(), 1);
    }

    /// The wake of a cold box waits for the fetch without holding the wake jobs: while the
    /// daemon says the box is on its way the job stays, the box reads waking, nobody is put to
    /// sleep for it and it is not started. Once it is back the same job wakes it. A fetch that
    /// failed ends the wake with the daemon's reason.
    #[tokio::test]
    async fn the_wake_of_a_cold_box_waits_for_its_fetch_and_fails_with_the_reason() {
        let (state, fake, user) = relay().await;
        asleep_for(&state, &user, DAY).await;
        record(&state, &user, NODE, "cold", None);
        let wakes = || calls(&fake, "/wake").len();
        let statuses = calls(&fake, "/status").len();
        fake.answer("prefetch", 200, json!({ "cold": true }));
        let e = boxes::start_box(&state, &user).await.unwrap_err();
        assert_eq!(e.to_string(), FETCHING);

        state.set_box_state(&user, BoxState::Asleep, None).await;
        boxes::ensure_awake(&state, &user, "send").await;
        for _ in 0..5 {
            let _ = state.db.retry_wake_job(&user, -1, "due");
            boxes::run_wake_jobs(&state).await;
            assert_eq!(state.box_state(&user).await.0, BoxState::Waking);
        }
        assert_eq!(calls(&fake, "/prefetch").len(), 6, "every pass asked");
        assert_eq!((wakes(), calls(&fake, "/status").len()), (1, statuses));
        assert_eq!(state.db.box_lifecycle(&user).unwrap().unwrap().1, "cold");

        fake.answer("prefetch", 200, json!({ "cold": false }));
        let _ = state.db.retry_wake_job(&user, -1, "due");
        boxes::run_wake_jobs(&state).await;
        assert_eq!(wakes(), 2);
        assert_eq!(state.db.box_lifecycle(&user).unwrap().unwrap().1, "warm");

        boxes::stop_box(&state, &user, false).await.unwrap();
        record(&state, &user, NODE, "cold", None);
        let why = "the box is cold and its work did not come back: object store GET boxes/u/work/00000: 503 Service Unavailable";
        fake.answer("prefetch", 502, json!({ "error": why }));
        let e = boxes::start_box(&state, &user)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("its work did not come back") && e.contains("503"),
            "{e}"
        );
        assert_eq!(wakes(), 2, "not started on a disk that did not come back");
    }

    /// The daemon's word for a box left in the object store is kept, and only the documented
    /// shapes of the two new replies are taken.
    #[tokio::test]
    async fn the_cold_replies_are_validated_and_a_cold_sleep_is_recorded() {
        let (state, fake, user) = relay().await;
        boxes::create_box(
            &state,
            &mut state.restart.admit_creation(&user).await,
            Provider::Boxd,
            None,
        )
        .await
        .unwrap();
        fake.answer(
            "sleep",
            200,
            json!({ "state": "cold", "files_bytes": null, "scratch_bytes": null,
                "trash_bytes": null, "work_bytes": 0, "ckpt_bytes": 0 }),
        );
        boxes::stop_box(&state, &user, true).await.unwrap();
        assert_eq!(state.db.box_lifecycle(&user).unwrap().unwrap().1, "cold");

        let boxd = &state.cfg.boxd.as_ref().unwrap().nodes[0];
        for bad in [
            json!({ "archived": true }),
            json!({ "archived": "yes", "stored_bytes": 1 }),
            json!({ "archived": true, "stored_bytes": -1 }),
            json!({ "archived": true, "stored_bytes": 1, "key": "x" }),
        ] {
            fake.answer("archive", 200, bad.clone());
            assert!(boxd.archive(&user, Part::Work).await.is_err(), "{bad}");
        }
        for bad in [
            json!({}),
            json!({ "cold": 1 }),
            json!({ "cold": true, "x": 1 }),
        ] {
            fake.answer("prefetch", 200, bad.clone());
            assert!(boxd.prefetch(&user).await.is_err(), "{bad}");
        }
        assert!(boxd.archive("a/b", Part::Ckpt).await.is_err());
        assert!(boxd.prefetch("..").await.is_err());
    }

    /// `GET /machine` of a cold box: where it was left and what its disk held, from the row.
    /// The daemon is not asked, the box is not woken, and a box of another provider answers
    /// what it always did.
    #[tokio::test]
    async fn the_machine_card_of_a_cold_box_is_served_from_the_row_and_wakes_nothing() {
        let (state, fake, user) = relay().await;
        let (_, device) = person(&state, 9700);
        let machine = |state: &AppState| {
            let (state, device) = (state.clone(), device.clone());
            async move {
                let reply = crate::api::machine(State(state), device).await.unwrap();
                let bytes = axum::body::to_bytes(reply.into_body(), 1 << 20)
                    .await
                    .unwrap();
                serde_json::from_slice::<Value>(&bytes).unwrap()
            }
        };
        let none = machine(&state).await;
        assert!(none.get("lifecycle_state").is_none() && none.get("storage").is_none());

        asleep_for(&state, &user, DAY).await;
        record(&state, &user, NODE, "cold", None);
        let asked = fake.paths().len();
        let cold = machine(&state).await;
        assert_eq!(cold["state"], "asleep");
        assert_eq!(cold["lifecycle_state"], "cold");
        let at = cold["storage"]["at"].as_i64().unwrap();
        assert_eq!(
            cold["storage"],
            json!({ "files_bytes": 1, "scratch_bytes": 2, "trash_bytes": null, "at": at })
        );
        assert_eq!(fake.paths().len(), asked, "the daemon was not asked");
        assert!(state.db.due_wake_jobs().unwrap().is_empty());
        assert_eq!(state.box_state(&user).await.0, BoxState::Asleep);

        let mut row = state.db.user_box(&user).unwrap().unwrap();
        row.provider = "gvisor".into();
        row.user_id = "u_gvisor".into();
        state.db.upsert_box(&row).unwrap();
        assert_eq!(state.db.box_lifecycle("u_gvisor").unwrap(), None);
    }

    /// Typing in an agent's chat wakes its sleeping box, and only that: not a box that is up,
    /// one its user stopped, one of another provider, or anything with the provider off.
    #[tokio::test]
    async fn typing_wakes_a_sleeping_box_and_leaves_a_stopped_one_down() {
        let (mut state, fake, user) = relay().await;
        let project = crate::people::tests_support::project_of(&state, &user, "Typing");
        boxes::create_box(
            &state,
            &mut state.restart.admit_creation(&user).await,
            Provider::Boxd,
            None,
        )
        .await
        .unwrap();
        state.set_box_state(&user, BoxState::Awake, None).await;
        wake_for_typing(&state, &project).await;
        assert_eq!(state.box_state(&user).await.0, BoxState::Awake);
        assert!(state.db.due_wake_jobs().unwrap().is_empty());

        // The user's own Stop holds it down: typing is not sending.
        boxes::stop_box(&state, &user, true).await.unwrap();
        state.db.set_box_held(&user, true).unwrap();
        wake_for_typing(&state, &project).await;
        assert_eq!(state.box_state(&user).await.0, BoxState::Asleep);

        state.db.set_box_held(&user, false).unwrap();
        wake_for_typing(&state, "no-such-project").await;
        assert_eq!(state.box_state(&user).await.0, BoxState::Asleep);
        let before = fake.paths().len();
        wake_for_typing(&state, &project).await;
        assert_eq!(state.box_state(&user).await.0, BoxState::Waking);
        let jobs = state.db.due_wake_jobs().unwrap();
        assert_eq!((jobs.len(), jobs[0].1.as_str()), (1, "typing"));
        // The wake itself is the wake job's, as for a message.
        assert_eq!(fake.paths().len(), before);
        boxes::run_wake_jobs(&state).await;
        assert_eq!(
            fake.paths().last().unwrap(),
            &format!("/v1/boxes/{user}/wake")
        );

        boxes::stop_box(&state, &user, false).await.unwrap();
        Arc::get_mut(&mut state.cfg).expect("one holder").boxd = None;
        wake_for_typing(&state, &project).await;
        assert_eq!(state.box_state(&user).await.0, BoxState::Asleep);
    }

    // ---- more than one node (Phase 4) --------------------------------------------------------

    #[test]
    fn a_new_box_goes_to_the_node_with_the_most_room() {
        let load = |free_slots, boxes, disk_free_gb| {
            Some(Load {
                free_slots,
                max_resident: 160,
                boxes,
                disk_free_gb,
            })
        };
        assert_eq!(place(&[]), None);
        let slots_first = [load(5, 10, 900.0), load(9, 900, 100.0)];
        assert_eq!(place(&slots_first), Some(1));
        let then_the_emptier = [load(5, 700, 900.0), load(5, 300, 100.0)];
        assert_eq!(place(&then_the_emptier), Some(1));
        let then_disk = [load(5, 300, 100.0), load(5, 300, 900.0)];
        assert_eq!(place(&then_disk), Some(1));
        let then_the_first_listed = [load(5, 300, 900.0), load(5, 300, 900.0)];
        assert_eq!(place(&then_the_first_listed), Some(0));
        // The emptier is the one with fewer boxes for its cap, not fewer boxes.
        let small = Some(Load {
            free_slots: 0,
            max_resident: 40,
            boxes: 100,
            disk_free_gb: 500.0,
        });
        assert_eq!(place(&[small, load(0, 300, 500.0)]), Some(1));
        // A node that did not answer, or is under the disk floor, takes no new box however
        // empty it is; a full one still does (its box takes an idle one's place).
        assert_eq!(place(&[None, load(1, 0, 80.0)]), Some(1));
        assert_eq!(place(&[load(160, 0, 79.9), load(0, 900, 80.0)]), Some(1));
        assert_eq!(place(&[load(160, 0, 79.9), None]), None);
    }

    /// A relay with two nodes, each a fake daemon with its own cap and its own box subnet, and
    /// three users.
    async fn two_nodes(caps: [usize; 2]) -> (AppState, [Arc<Fake>; 2], [String; 3]) {
        let (fake0, mut node0) = served(TOKEN).await;
        let (fake1, mut node1) = served_as("node-1", TOKEN).await;
        (node0.max_resident, node1.max_resident) = (caps[0], caps[1]);
        node1.subnet = [10, 208];
        let there =
            json!({ "state": "warm", "restored": false, "box_url": "http://10.208.0.2:7788" });
        fake1.answer("wake", 200, there);
        let boxd = Boxd {
            nodes: vec![node0, node1],
            wanted: Arc::default(),
        };
        let mut state = test_state(AuthMode::Github);
        Arc::get_mut(&mut state.cfg).expect("one holder").boxd = Some(boxd);
        let users = [9710, 9711, 9712].map(|id| person(&state, id).0);
        (state, [fake0, fake1], users)
    }

    /// What a fake was asked for one user, by operation.
    fn asked(fake: &Fake, user: &str) -> Vec<String> {
        let of_user = format!("/v1/boxes/{user}/");
        let paths = fake.paths();
        let ops = paths.iter().filter_map(|p| p.strip_prefix(&of_user));
        ops.map(str::to_owned).collect()
    }

    async fn make_awake(state: &AppState, user: &str) {
        boxes::create_box(
            state,
            &mut state.restart.admit_creation(user).await,
            Provider::Boxd,
            None,
        )
        .await
        .unwrap();
        state.set_box_state(user, BoxState::Awake, None).await;
    }

    fn node_on_row(state: &AppState, user: &str) -> String {
        state.db.box_lifecycle(user).unwrap().expect("a row").0
    }

    /// Placement follows the free resident slots, the row says where the box went, and every
    /// later call for that box goes to that node and to no other.
    #[tokio::test]
    async fn a_box_is_placed_once_and_every_later_call_goes_to_its_node() {
        let (state, [fake0, fake1], [a, b, c]) = two_nodes([2, 3]).await;
        // 2 against 3 free: node-1. Then 2 against 2, and each daemon lists one box: node-1 has
        // the fewer for its cap. Then 2 against 1.
        make_awake(&state, &a).await;
        make_awake(&state, &b).await;
        make_awake(&state, &c).await;
        let placed = [&a, &b, &c].map(|u| node_on_row(&state, u));
        assert_eq!(placed, ["node-1", "node-1", "node-0"]);
        assert_eq!(state.db.boxd_node(&a).unwrap().as_deref(), Some("node-1"));
        let row = state.db.user_box(&a).unwrap().unwrap();
        assert_eq!(row.box_url.as_deref(), Some("http://10.208.0.2:7788"));

        boxes::stop_box(&state, &a, false).await.unwrap();
        boxes::start_box(&state, &a).await.unwrap();
        assert_eq!(asked(&fake1, &a), ["ensure", "wake", "sleep", "wake"]);
        assert_eq!(asked(&fake0, &a), [] as [&str; 0]);
        assert_eq!(asked(&fake0, &c), ["ensure", "wake"]);
        assert_eq!(asked(&fake1, &c), [] as [&str; 0]);
        assert_eq!(node_on_row(&state, &a), "node-1");
        assert_eq!(
            boxes::provider_status(&state, &a).await["node_id"],
            "node-1"
        );

        let cap = capacity(&state).await.unwrap();
        assert_eq!((cap.max_resident, cap.resident, cap.has_room), (5, 3, true));
        let per_node: Vec<(&str, usize)> = cap
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), n.resident))
            .collect();
        assert_eq!(per_node, [("node-0", 1), ("node-1", 2)]);
    }

    /// The cap is each node's own: a box that wakes on a full node takes the place of an idle
    /// box on that node, and a box on another node is not the one, however long it has idled.
    #[tokio::test]
    async fn a_full_node_makes_room_among_its_own_boxes() {
        let (state, [fake0, fake1], [a, b, c]) = two_nodes([1, 1]).await;
        make_awake(&state, &a).await;
        make_awake(&state, &b).await;
        assert_eq!(
            [node_on_row(&state, &a), node_on_row(&state, &b)],
            ["node-0", "node-1"]
        );
        // `b` has idled longest of all, and is on the other node.
        state
            .last_client_push
            .lock()
            .await
            .insert(a.clone(), now_ms() + 60_000);
        make_awake(&state, &c).await;
        assert_eq!(node_on_row(&state, &c), "node-0");
        assert_eq!(asked(&fake0, &a), ["ensure", "wake", "sleep"]);
        assert_eq!(asked(&fake1, &b), ["ensure", "wake"]);
        assert_eq!(state.box_state(&b).await.0, BoxState::Awake);
    }

    /// A node that is down: its boxes say so and stay its boxes, a new box goes to a node that
    /// is up, and a user whose disk is on the node that is down is not given an empty one
    /// elsewhere, with or without a row.
    #[tokio::test]
    async fn a_node_that_is_down_keeps_its_boxes_and_takes_no_new_one() {
        let (mut state, [fake0, _fake1], [a, b, _]) = two_nodes([2, 3]).await;
        make_awake(&state, &a).await;
        boxes::stop_box(&state, &a, false).await.unwrap();
        assert_eq!(node_on_row(&state, &a), "node-1");
        // Nothing listens on port 1.
        let cfg = Arc::get_mut(&mut state.cfg).expect("one holder");
        cfg.boxd.as_mut().unwrap().nodes[1].url = "http://127.0.0.1:1".into();

        let down = boxes::start_box(&state, &a).await.unwrap_err().to_string();
        assert!(down.contains("node-1 did not answer"), "{down}");
        assert_eq!(node_on_row(&state, &a), "node-1");
        let down = boxes::stop_box(&state, &a, true).await.unwrap_err();
        assert!(down.to_string().contains("node-1 did not answer"), "{down}");

        make_awake(&state, &b).await;
        assert_eq!(node_on_row(&state, &b), "node-0");
        let cap = capacity(&state).await.unwrap();
        let up: Vec<bool> = cap.nodes.iter().map(|n| n.up).collect();
        assert_eq!(
            (up, cap.max_resident, cap.has_room),
            (vec![true, false], 2, true)
        );

        // An admin's delete drops the row and leaves the disk; the next box is made where the
        // disk is, or not at all.
        state.db.delete_box_row(&a).unwrap();
        let e = boxes::create_box(
            &state,
            &mut state.restart.admit_creation(&a).await,
            Provider::Boxd,
            None,
        )
        .await;
        assert!(e.unwrap_err().to_string().contains("node-1 did not answer"));
        assert!(state.db.user_box(&a).unwrap().is_none());
        assert_eq!(
            asked(&fake0, &a),
            [] as [&str; 0],
            "nothing was moved or made anew"
        );

        // Nor is a box sent to a node this relay no longer lists.
        let cfg = Arc::get_mut(&mut state.cfg).expect("one holder");
        cfg.boxd.as_mut().unwrap().nodes.pop();
        let e = boxes::create_box(
            &state,
            &mut state.restart.admit_creation(&a).await,
            Provider::Boxd,
            None,
        )
        .await;
        assert!(e.unwrap_err().to_string().contains("node-1 is not one"));
        assert_eq!(asked(&fake0, &a), [] as [&str; 0]);
    }

    /// A user with no node on record whose disk a node already lists (linked in by the
    /// operator's move off Docker) is made there, whatever room the others have; listed by two
    /// nodes, nowhere.
    #[tokio::test]
    async fn a_disk_already_on_a_node_decides_where_its_box_is_made() {
        let (state, [fake0, fake1], [a, ..]) = two_nodes([160, 1]).await;
        let listing = |user: &str| {
            let mut status = usual("status").1;
            status["boxes"][0]["user_id"] = json!(user);
            status["boxes"][0]["running"] = json!(false);
            status
        };
        fake1.answer("status", 200, listing(&a));
        make_awake(&state, &a).await;
        assert_eq!(node_on_row(&state, &a), "node-1");
        assert_eq!(asked(&fake0, &a), [] as [&str; 0]);

        let (state, [fake0, fake1], [a, ..]) = two_nodes([160, 160]).await;
        fake0.answer("status", 200, listing(&a));
        fake1.answer("status", 200, listing(&a));
        let e = boxes::create_box(
            &state,
            &mut state.restart.admit_creation(&a).await,
            Provider::Boxd,
            None,
        )
        .await;
        assert!(e.unwrap_err().to_string().contains("more than one node"));
        assert_eq!(
            [asked(&fake0, &a), asked(&fake1, &a)],
            [vec![], vec![] as Vec<String>]
        );
        assert!(state.db.user_box(&a).unwrap().is_none());
    }

    /// The pass that archives looks at each node's own disk and sends each box to its own node.
    #[tokio::test]
    async fn each_node_archives_its_own_sleeping_boxes() {
        let (state, [fake0, fake1], [a, b, _]) = two_nodes([3, 3]).await;
        make_awake(&state, &a).await;
        make_awake(&state, &b).await;
        for (fake, user) in [(&fake0, &a), (&fake1, &b)] {
            boxes::stop_box(&state, user, false).await.unwrap();
            let mut row = state.db.user_box(user).unwrap().unwrap();
            row.last_stopped_at = Some(now_ms() - CKPT_COLD_MS - 1);
            state.db.upsert_box(&row).unwrap();
            // Both hosts have a store, and each lists its own box asleep with a checkpoint.
            let mut status = usual("status").1;
            status["cold_tier"] = json!(true);
            status["boxes"][0] = json!({ "user_id": user, "running": false, "size_gb": 8,
                "work_bytes": 4096, "ckpt_bytes": 7 });
            fake.answer("status", 200, status);
        }
        tier_tick(&state).await;
        assert_eq!(asked(&fake0, &a).last().unwrap(), "archive");
        assert_eq!(asked(&fake1, &b).last().unwrap(), "archive");
        assert!(!asked(&fake1, &a).contains(&"archive".to_owned()));
        assert!(!asked(&fake0, &b).contains(&"archive".to_owned()));
    }

    /// A CA of two days in `dir` as `<ca>.pem`, and for each name a key and a certificate it
    /// signed for 127.0.0.1. Made by openssl(1), which every host these tests run on has.
    fn pki(dir: &std::path::Path, ca: &str, names: &[&str]) {
        std::fs::create_dir_all(dir).unwrap();
        let at = |file: String| dir.join(file).to_str().unwrap().to_owned();
        let openssl = |args: &[&str]| {
            let out = std::process::Command::new("openssl")
                .args(args)
                .output()
                .expect("openssl on PATH");
            assert!(out.status.success(), "openssl {args:?}: {out:?}");
        };
        let key = |name: &str| {
            let path = at(format!("{name}.key"));
            openssl(&[
                "ecparam",
                "-name",
                "prime256v1",
                "-genkey",
                "-noout",
                "-out",
                &path,
            ]);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            path
        };
        let (ca_key, ca_pem) = (key(ca), at(format!("{ca}.pem")));
        let subject = format!("/CN={ca}");
        openssl(&[
            "req", "-x509", "-new", "-key", &ca_key, "-out", &ca_pem, "-subj", &subject, "-days",
            "2",
        ]);
        let ext = at(format!("{ca}.ext"));
        std::fs::write(&ext, "subjectAltName=IP:127.0.0.1\n").unwrap();
        for name in names {
            let (key, csr, pem) = (
                key(name),
                at(format!("{name}.csr")),
                at(format!("{name}.pem")),
            );
            let subject = format!("/CN={name}");
            openssl(&["req", "-new", "-key", &key, "-out", &csr, "-subj", &subject]);
            openssl(&[
                "x509",
                "-req",
                "-in",
                &csr,
                "-CA",
                &ca_pem,
                "-CAkey",
                &ca_key,
                "-CAcreateserial",
                "-out",
                &pem,
                "-days",
                "2",
                "-extfile",
                &ext,
            ]);
        }
    }

    /// The fake behind what `arbos-boxd` opens for another host: TLS 1.3, `node.pem`, and a
    /// certificate from `ca.pem` asked of every caller. Returns its `https://` address.
    async fn served_tls(dir: &std::path::Path) -> (Arc<Fake>, String) {
        use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
        struct Door(tokio::net::TcpListener, tokio_rustls::TlsAcceptor);
        impl axum::serve::Listener for Door {
            type Io = tokio_rustls::server::TlsStream<tokio::net::TcpStream>;
            type Addr = SocketAddr;
            async fn accept(&mut self) -> (Self::Io, Self::Addr) {
                loop {
                    let (stream, peer) = self.0.accept().await.unwrap();
                    if let Ok(tls) = self.1.accept(stream).await {
                        return (tls, peer);
                    }
                }
            }
            fn local_addr(&self) -> std::io::Result<Self::Addr> {
                self.0.local_addr()
            }
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut roots = rustls::RootCertStore::empty();
        let ca = CertificateDer::from_pem_file(dir.join("ca.pem")).unwrap();
        roots.add(ca).unwrap();
        let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
            Arc::new(roots),
            provider.clone(),
        );
        let config = rustls::ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_client_cert_verifier(verifier.build().unwrap())
            .with_single_cert(
                vec![CertificateDer::from_pem_file(dir.join("node.pem")).unwrap()],
                PrivateKeyDer::from_pem_file(dir.join("node.key")).unwrap(),
            )
            .unwrap();
        let fake = Arc::new(Fake::default());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("https://{}", l.local_addr().unwrap());
        let app = axum::Router::new()
            .fallback(answer)
            .with_state(fake.clone());
        let door = Door(l, tokio_rustls::TlsAcceptor::from(Arc::new(config)));
        tokio::spawn(async move { axum::serve(door, app).await });
        (fake, url)
    }

    /// A node on another host is dialled over TLS that both ends prove. A node whose
    /// certificate is from another CA is not sent the bearer; a relay whose certificate is from
    /// another CA is not answered; and no node is configured without all of it in order.
    #[tokio::test]
    async fn a_node_on_another_host_is_reached_only_over_mutual_tls() {
        let dir = std::env::temp_dir().join(format!("arbos-relay-boxd-tls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        pki(&dir, "ca", &["node", "relay"]);
        pki(&dir, "other", &["stranger"]);
        let file = |name: &str| dir.join(name).to_str().unwrap().to_owned();
        std::fs::write(file("token"), TOKEN).unwrap();
        std::fs::set_permissions(file("token"), std::fs::Permissions::from_mode(0o600)).unwrap();
        let (fake, url) = served_tls(&dir).await;
        let with = |url: &str, ca: &str, cert: &str, key: &str| NodeCfg {
            ca_file: Some(file(ca)),
            cert_file: Some(file(cert)),
            key_file: Some(file(key)),
            box_subnet: Some("10.208.0.0/16".into()),
            ..cfg("node-1", url, &file("token"))
        };

        let node = Node::new(&with(&url, "ca.pem", "relay.pem", "relay.key")).unwrap();
        assert_eq!(node.host().await.unwrap().disk_free_gb, 900.0);
        // The address of a box is one of this node's, or the box's token is not sent there.
        let elsewhere = node.wake("u_a").await.unwrap_err().to_string();
        assert!(elsewhere.contains("subnet"), "{elsewhere}");
        let here =
            json!({ "state": "warm", "restored": false, "box_url": "http://10.208.0.2:7788" });
        fake.answer("wake", 200, here);
        assert_eq!(node.wake("u_a").await.unwrap(), "http://10.208.0.2:7788");
        let seen = fake.paths().len();

        // The node's certificate is not from the CA this relay was given.
        let node = Node::new(&with(&url, "other.pem", "relay.pem", "relay.key")).unwrap();
        let e = node.host().await.err().expect("refused");
        assert!(e.to_string().contains("node-1 did not answer"), "{e}");
        // The relay's certificate is not from the CA the node was given.
        let node = Node::new(&with(&url, "ca.pem", "stranger.pem", "stranger.key")).unwrap();
        assert!(node.host().await.is_err());
        // The node's certificate names 127.0.0.1 and not this name for it.
        let named = url.replace("127.0.0.1", "localhost");
        let node = Node::new(&with(&named, "ca.pem", "relay.pem", "relay.key")).unwrap();
        assert!(node.host().await.is_err());
        assert_eq!(fake.paths().len(), seen, "none of those reached a route");
        for e in [e.to_string(), format!("{e:#}")] {
            assert!(!e.contains(TOKEN), "{e}");
        }

        let plain = "http://10.0.0.5:7790";
        let pathed = format!("{url}/v1");
        let at_user = url.replace("https://", "https://user:pw@");
        let mut bad = vec![
            with(plain, "ca.pem", "relay.pem", "relay.key"),
            with("http://127.0.0.1:7790", "ca.pem", "relay.pem", "relay.key"),
            with(&pathed, "ca.pem", "relay.pem", "relay.key"),
            with(&at_user, "ca.pem", "relay.pem", "relay.key"),
            with(&url, "ca.pem", "relay.pem", "node.key"),
            with(&url, "relay.key", "relay.pem", "relay.key"),
            with(&url, "missing.pem", "relay.pem", "relay.key"),
            cfg("node-1", &url, &file("token")),
            cfg("node-1", plain, &file("token")),
            cfg("Node 1", "http://127.0.0.1:7790", &file("token")),
            NodeCfg {
                key_file: None,
                ..with(&url, "ca.pem", "relay.pem", "relay.key")
            },
            NodeCfg {
                max_resident: Some(0),
                ..with(&url, "ca.pem", "relay.pem", "relay.key")
            },
        ];
        for subnet in ["10.208.0.0/24", "10.208.1.0/16", "8.8.0.0/16", "10.208.0.0"] {
            bad.push(NodeCfg {
                box_subnet: Some(subnet.into()),
                ..with(&url, "ca.pem", "relay.pem", "relay.key")
            });
        }
        for (i, cfg) in bad.iter().enumerate() {
            assert!(Node::new(cfg).is_err(), "{i}: {}", cfg.url);
        }
        std::fs::set_permissions(file("relay.key"), std::fs::Permissions::from_mode(0o640))
            .unwrap();
        let loose = Node::new(&with(&url, "ca.pem", "relay.pem", "relay.key"));
        assert!(loose.err().unwrap().to_string().contains("0600"));
        std::fs::set_permissions(file("relay.key"), std::fs::Permissions::from_mode(0o600))
            .unwrap();
        std::fs::set_permissions(file("ca.pem"), std::fs::Permissions::from_mode(0o664)).unwrap();
        assert!(Node::new(&with(&url, "ca.pem", "relay.pem", "relay.key")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The nodes file is a list of nodes and nothing else, in a file only its owner can write;
    /// and no two nodes share an id, a daemon or a box subnet.
    #[test]
    fn the_nodes_file_is_strict_and_no_two_nodes_are_alike() {
        let dir =
            std::env::temp_dir().join(format!("arbos-relay-boxd-nodes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = |name: &str| dir.join(name).to_str().unwrap().to_owned();
        let write = |name: &str, text: &str, mode: u32| {
            std::fs::write(file(name), text).unwrap();
            std::fs::set_permissions(file(name), std::fs::Permissions::from_mode(mode)).unwrap();
        };
        write("token", TOKEN, 0o600);
        let entry = |id: &str, port: u16, subnet: &str| {
            json!({ "id": id, "url": format!("http://127.0.0.1:{port}"),
                "token_file": file("token"), "box_subnet": subnet, "max_resident": 40 })
        };
        let list = json!([entry("node-1", 7791, "10.208.0.0/16")]).to_string();
        write("nodes.json", &list, 0o644);
        let more = nodes_in(&file("nodes.json")).unwrap();
        let first = cfg(NODE, "http://127.0.0.1:7790", &file("token"));
        let boxd = Boxd::new(&[first, more.into_iter().next().unwrap()]).unwrap();
        let read: Vec<_> = boxd
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), n.max_resident, n.subnet))
            .collect();
        assert_eq!(
            read,
            [("node-0", 160, [10, 207]), ("node-1", 40, [10, 208])]
        );
        assert!(boxd.node("node-1").is_ok() && boxd.node("node-2").is_err());

        write("nodes.json", &list, 0o666);
        assert!(
            nodes_in(&file("nodes.json")).is_err(),
            "others can write it"
        );
        let mut extra = entry("node-1", 7791, "10.208.0.0/16");
        extra["insecure"] = json!(true);
        write("nodes.json", &json!([extra]).to_string(), 0o644);
        assert!(
            nodes_in(&file("nodes.json")).is_err(),
            "a field nobody defined"
        );
        write("nodes.json", "{}", 0o644);
        assert!(nodes_in(&file("nodes.json")).is_err());

        for (id, port, subnet) in [
            ("node-0", 7791, "10.208.0.0/16"),
            ("node-1", 7790, "10.208.0.0/16"),
            ("node-1", 7791, "10.207.0.0/16"),
        ] {
            let second: NodeCfg = serde_json::from_value(entry(id, port, subnet)).unwrap();
            let first = cfg(NODE, "http://127.0.0.1:7790", &file("token"));
            assert!(Boxd::new(&[first, second]).is_err(), "{id} {port} {subnet}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A wake job is run when it is queued: the runner does not sit out the rest of its
    /// 750 ms look first.
    #[tokio::test]
    async fn a_queued_wake_reaches_the_daemon_at_once() {
        let (state, fake, user) = relay().await;
        boxes::create_box(
            &state,
            &mut state.restart.admit_creation(&user).await,
            Provider::Boxd,
            None,
        )
        .await
        .unwrap();
        state.set_box_state(&user, BoxState::Awake, None).await;
        boxes::stop_box(&state, &user, false).await.unwrap();
        boxes::spawn_wake_runner(state.clone()).await;
        // The runner has found nothing and is 100 ms into its wait.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let before = fake.paths().len();
        boxes::ensure_awake(&state, &user, "send").await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let wake = format!("/v1/boxes/{user}/wake");
        assert!(fake.paths()[before..].contains(&wake), "{:?}", fake.paths());
    }

    /// Wakes run side by side and a node is never over its cap. Forty boxes on a node that
    /// holds twelve, as in the run that once had 43 awake under that cap: the twelve that are
    /// awake sit idle, and the other twenty-eight are woken in one pass of the runner. Each
    /// needs an idle box's slot, so twelve wake and the rest are told the host is full.
    #[tokio::test(flavor = "multi_thread")]
    async fn boxes_woken_at_once_never_put_a_node_over_its_cap() {
        const CAP: usize = 12;
        let (mut state, fake, first) = relay().await;
        let cfg = Arc::get_mut(&mut state.cfg).expect("one holder");
        cfg.boxd.as_mut().unwrap().nodes[0].max_resident = CAP;
        let mut users = vec![first];
        users.extend((0..39).map(|i| person(&state, 9800 + i).0));
        // Made one after another: from the thirteenth on, each takes the place of an idle box.
        for user in &users {
            make_awake(&state, user).await;
        }
        let mut asleep = Vec::new();
        for user in &users {
            if state.box_state(user).await.0 == BoxState::Asleep {
                asleep.push(user.clone());
            }
        }
        assert_eq!(asleep.len(), users.len() - CAP);
        assert_eq!(fake.up.lock().unwrap().len(), CAP);

        *fake.takes.lock().unwrap() = Duration::from_millis(20);
        let before = fake.paths().len();
        for user in &asleep {
            boxes::ensure_awake(&state, user, "send").await;
        }
        boxes::run_wake_jobs(&state).await;

        let paths = fake.paths()[before..].to_vec();
        let woken: HashSet<&String> = paths.iter().filter(|p| p.ends_with("/wake")).collect();
        assert_eq!(woken.len(), CAP, "one wake for each idle box put to sleep");
        assert_eq!(paths.iter().filter(|p| p.ends_with("/wake")).count(), CAP);
        assert_eq!(paths.iter().filter(|p| p.ends_with("/sleep")).count(), CAP);
        let most_up = fake.most_up.load(Ordering::SeqCst);
        assert!(most_up <= CAP, "{most_up} boxes up under a cap of {CAP}");
        assert_eq!(fake.up.lock().unwrap().len(), CAP);
        let at_once = fake.most_waking.load(Ordering::SeqCst);
        assert!(
            (2..=WAKES_PER_NODE).contains(&at_once),
            "{at_once} wakes at once"
        );
        // Nothing is held once the wakes are over, and the ones turned away know why.
        let node = &state.cfg.boxd.as_ref().unwrap().nodes[0];
        let slots = node.slots.lock().unwrap();
        assert!(slots.coming.is_empty() && slots.going.is_empty());
        assert_eq!(node.wakes.available_permits(), WAKES_PER_NODE);
        let full = state.db.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM box_wake_jobs WHERE last_error = ?1",
                [FULL],
                |r| r.get::<_, i64>(0),
            )
        });
        assert_eq!(full.unwrap() as usize, asleep.len() - CAP);
    }

    /// A box can dial in before the daemon's answer to its wake is back. It is awake then and
    /// idle, and not yet another box's to take the place of.
    #[tokio::test]
    async fn a_box_on_its_way_up_is_not_put_to_sleep_for_another() {
        let (mut state, fake, a) = relay().await;
        let cfg = Arc::get_mut(&mut state.cfg).expect("one holder");
        cfg.boxd.as_mut().unwrap().nodes[0].max_resident = 1;
        let (b, _) = person(&state, 9701);
        for user in [&a, &b] {
            make_awake(&state, user).await;
            boxes::stop_box(&state, user, false).await.unwrap();
        }
        *fake.takes.lock().unwrap() = Duration::from_millis(200);
        let waking = {
            let (state, a) = (state.clone(), a.clone());
            tokio::spawn(async move { boxes::start_box(&state, &a).await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        state.set_box_state(&a, BoxState::Awake, None).await;
        let before = asked(&fake, &a);
        let full = boxes::start_box(&state, &b).await.unwrap_err();
        assert_eq!(full.to_string(), FULL);
        assert_eq!(asked(&fake, &a), before, "nothing more was asked of it");
        waking.await.unwrap().unwrap();
    }

    /// A relay whose one node has twelve slots, so a headroom of three, and an idle box awake
    /// in every one: the first of them the longest idle, the last the least.
    async fn twelve_idle() -> (AppState, Arc<Fake>, Vec<String>) {
        let (mut state, fake, first) = relay().await;
        let cfg = Arc::get_mut(&mut state.cfg).expect("one holder");
        cfg.boxd.as_mut().unwrap().nodes[0].max_resident = 12;
        let mut users = vec![first];
        users.extend((0..11).map(|i| person(&state, 9900 + i).0));
        for (i, user) in users.iter().enumerate() {
            make_awake(&state, user).await;
            let mut row = state.db.user_box(user).unwrap().unwrap();
            row.last_started_at = Some(1_000 + i as i64);
            state.db.upsert_box(&row).unwrap();
        }
        (state, fake, users)
    }

    fn slept(fake: &Fake, users: &[String]) -> Vec<bool> {
        let asked = |u: &String| asked(fake, u).iter().any(|op| op == "sleep");
        users.iter().map(asked).collect()
    }

    /// A node keeps slots free, so a box that wakes on a node at its cap takes one at once: no
    /// box is put to sleep ahead of its wake. The slot is made up behind it.
    #[tokio::test]
    async fn a_wake_on_a_full_node_takes_a_slot_kept_free_and_waits_for_no_sleep() {
        let (state, fake, users) = twelve_idle().await;
        let node = &state.cfg.boxd.as_ref().unwrap().nodes[0];
        assert_eq!(node.headroom(), 3);
        keep_headroom(&state, node).await;
        let gone = slept(&fake, &users);
        assert_eq!(gone[..4], [true, true, true, false], "the longest idle");
        assert_eq!(gone.iter().filter(|g| **g).count(), 3);
        // Nothing more while the headroom is there.
        let before = fake.paths().len();
        keep_headroom(&state, node).await;
        assert_eq!(fake.paths().len(), before, "the daemon is asked nothing");

        boxes::start_box(&state, &users[0]).await.unwrap();
        let asked_for: Vec<String> = fake.paths()[before..].to_vec();
        assert!(
            asked_for.iter().all(|p| !p.ends_with("/sleep")),
            "{asked_for:?}"
        );
        assert!(asked_for.contains(&format!("/v1/boxes/{}/wake", users[0])));
        // The slot it took is made up from the next longest idle.
        keep_headroom(&state, node).await;
        assert_eq!(slept(&fake, &users)[3..5], [true, false]);
        assert!(fake.most_up.load(Ordering::SeqCst) <= 12);
    }

    /// Wakes that arrive together do not count each other: five on a node with three slots
    /// free take those three, and only the two left over have a box put to sleep for them.
    #[tokio::test]
    async fn a_burst_of_wakes_takes_the_free_slots_before_any_box_is_put_to_sleep() {
        let (mut state, fake, first) = relay().await;
        let cfg = Arc::get_mut(&mut state.cfg).expect("one holder");
        cfg.boxd.as_mut().unwrap().nodes[0].max_resident = 12;
        let mut users = vec![first];
        users.extend((0..13).map(|i| person(&state, 9800 + i).0));
        let (asleep, awake) = users.split_at(5);
        for user in asleep {
            make_awake(&state, user).await;
            boxes::stop_box(&state, user, false).await.unwrap();
        }
        for user in awake {
            make_awake(&state, user).await;
        }
        *fake.takes.lock().unwrap() = Duration::from_millis(100);
        let before = fake.paths().len();
        for user in asleep {
            state.set_box_state(user, BoxState::Waking, None).await;
        }
        let wakes = asleep.iter().map(|user| boxes::start_box(&state, user));
        for woken in futures::future::join_all(wakes).await {
            woken.unwrap();
        }
        let sleeps = fake.paths()[before..]
            .iter()
            .filter(|p| p.ends_with("/sleep"))
            .count();
        assert_eq!(sleeps, 2, "three slots were free for five wakes");
        assert!(fake.most_up.load(Ordering::SeqCst) <= 12);
    }

    /// The headroom is made of idle boxes only: one with a turn running is passed over for
    /// the next, and one agentd calls busy stays up with nothing asked of it twice.
    #[tokio::test]
    async fn the_headroom_never_takes_a_busy_box() {
        let (state, fake, users) = twelve_idle().await;
        let node = &state.cfg.boxd.as_ref().unwrap().nodes[0];
        let mut turns = state.turn_running.lock().await;
        for user in &users[..3] {
            turns.insert(user.clone(), now_ms());
        }
        drop(turns);
        keep_headroom(&state, node).await;
        let gone = slept(&fake, &users);
        assert_eq!(gone[..7], [false, false, false, true, true, true, false]);

        // The three after those are busy by agentd's account: asked once each, left awake.
        boxes::start_box(&state, &users[3]).await.unwrap();
        fake.answer("sleep", 409, json!({ "error": "busy: a job is running" }));
        let before = fake.paths().len();
        keep_headroom(&state, node).await;
        let sleeps = |user: &String| {
            let asked = asked(&fake, user);
            asked.iter().filter(|op| *op == "sleep").count()
        };
        let asked_once = fake.paths()[before..]
            .iter()
            .filter(|p| p.ends_with("/sleep"))
            .count();
        assert_eq!(asked_once, 6, "each idle box once, and none twice");
        for user in &users[..3] {
            assert_eq!(sleeps(user), 0, "a turn is running");
        }
        for user in &users[6..] {
            assert_eq!(state.box_state(user).await.0, BoxState::Awake);
        }
    }

    /// The cap holds with the headroom kept behind a burst: forty boxes on twelve slots, the
    /// sleeping ones all woken at once while the node makes room.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_cap_holds_while_the_headroom_is_kept_behind_a_burst() {
        let (mut state, fake, first) = relay().await;
        let cfg = Arc::get_mut(&mut state.cfg).expect("one holder");
        cfg.boxd.as_mut().unwrap().nodes[0].max_resident = 12;
        let mut users = vec![first];
        users.extend((0..39).map(|i| person(&state, 9950 + i).0));
        for user in &users {
            make_awake(&state, user).await;
        }
        *fake.takes.lock().unwrap() = Duration::from_millis(20);
        spawn_headroom(&state);
        for user in &users {
            boxes::ensure_awake(&state, user, "send").await;
        }
        boxes::run_wake_jobs(&state).await;
        let most_up = fake.most_up.load(Ordering::SeqCst);
        assert!(most_up <= 12, "{most_up} boxes up under a cap of 12");
        let woken = fake.paths().iter().filter(|p| p.ends_with("/wake")).count();
        assert!(woken > 40, "the burst woke some: {woken}");
    }

    /// A chat on screen wakes the sleeping box behind it while its node is at or below its
    /// soft ceiling: not on a node above it, and not a box its user stopped.
    #[tokio::test]
    async fn a_chat_on_screen_wakes_its_box_only_on_a_node_with_room_to_spare() {
        let (state, _fake, users) = twelve_idle().await;
        let node = &state.cfg.boxd.as_ref().unwrap().nodes[0];
        let project = |i: usize| crate::people::tests_support::project_of(&state, &users[i], "p");
        let projects = [project(0), project(1), project(2)];
        // Twelve of twelve awake: above the soft ceiling of nine, and nothing is asleep yet.
        keep_headroom(&state, node).await;
        for user in &users[..3] {
            assert_eq!(state.box_state(user).await.0, BoxState::Asleep);
        }
        // Nine awake, at the soft ceiling. A stopped box stays down; a sleeping one comes up.
        state.db.set_box_held(&users[2], true).unwrap();
        wake_for_focus(&state, &projects[2]).await;
        assert_eq!(state.box_state(&users[2]).await.0, BoxState::Asleep);
        wake_for_focus(&state, &projects[0]).await;
        assert_eq!(state.box_state(&users[0]).await.0, BoxState::Waking);
        let jobs = state.db.due_wake_jobs().unwrap();
        assert_eq!((jobs.len(), jobs[0].1.as_str()), (1, "focus"));
        // Ten with a slot now, above the soft ceiling: the next look wakes nothing, typing does.
        wake_for_focus(&state, &projects[1]).await;
        assert_eq!(state.box_state(&users[1]).await.0, BoxState::Asleep);
        wake_for_typing(&state, &projects[1]).await;
        assert_eq!(state.box_state(&users[1]).await.0, BoxState::Waking);
    }
}
