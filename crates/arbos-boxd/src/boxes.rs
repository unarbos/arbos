//! The box state machine: ensure, wake, sleep, grow, and the host's status.
//!
//! A box is a directory, `<root>/boxes/<user_id>/`:
//!
//! - `work.img` — the user's disk, a sparse ext4 image (`/work`, `$HOME` and `/etc` live on it)
//! - `mnt/` — where it is mounted while the box is awake
//! - `config.json` — the OCI bundle (it holds the box's tokens, so it is 0600)
//! - `ckpt/` — a checkpoint, present only while the box sleeps with user processes
//! - `ckpt.cold.json`, `work.cold.json` — what the cold tier holds of the box (`cold.rs`)
//! - `ip` — the box's address on the bridge
//! - `box.log` — the sandbox's stdout and stderr
//!
//! Everything in that directory is root's except what is under `mnt/`, which the user's box
//! writes. So the two names read there (`.home`, `.etc`) are checked and never followed.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::ErrorKind;
use std::net::Ipv4Addr;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::cold::{Manifest, Store};
use crate::oci;
use crate::sys::{Out, Sys, AGENTD_PORT};

pub const GIB: u64 = 1 << 30;

/// The sizes a `work.img` moves through (docs/box-scaling.md §3.3).
pub const TIERS_GIB: [u64; 3] = [8, 32, 128];

/// The bridge every box's veth joins. It is `.0.1` of the host's /16 ([`Cfg::subnet`]).
const BRIDGE: &str = "arbos-boxd0";
/// The /16 of a host that is not told another.
pub const SUBNET: &str = "10.207.0.0/16";

/// `10.208.0.0/16` → `[10, 208]`. A private /16 and nothing else.
pub fn parse_subnet(s: &str) -> Option<[u8; 2]> {
    let ip: Ipv4Addr = s.strip_suffix("/16")?.parse().ok()?;
    let [a, b, 0, 0] = ip.octets() else {
        return None;
    };
    ip.is_private().then_some([a, b])
}

/// How long a clean stop waits for agentd after SIGTERM. agentd gives its own shutdown 45 s.
const STOP_GRACE: Duration = Duration::from_secs(50);

/// What the boxes resolve names with. The relay's `ARBOS_BOX_DNS` default.
const RESOLV_CONF: &str = "nameserver 1.1.1.1\nnameserver 8.8.8.8\noptions timeout:2 attempts:2\n";
const HOSTS: &str = "127.0.0.1\tlocalhost\n::1\tlocalhost ip6-localhost ip6-loopback\n";

/// A user id that is safe as one path segment. The relay's rule for a box path
/// (`fleet::safe_user_segment`): its ids are ULIDs, `test_<ulid>` and a few `u_<name>`.
#[derive(Debug, Clone, PartialEq)]
pub struct UserId(String);

impl UserId {
    pub fn parse(s: &str) -> Option<Self> {
        let ok = !s.is_empty()
            && s.len() <= 64
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        ok.then(|| Self(s.to_owned()))
    }
}

/// An image version is a directory name under `images/`, and is written into a shell script.
pub fn valid_version(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && !s.starts_with('.')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

/// The environment the relay hands a box: shell-style names, bounded, no control characters.
pub fn valid_env(env: &BTreeMap<String, String>) -> bool {
    let name = |k: &str| {
        !k.is_empty()
            && k.len() <= 128
            && !k.starts_with(|c: char| c.is_ascii_digit())
            && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    env.len() <= 64
        && env
            .iter()
            .all(|(k, v)| name(k) && v.len() <= 4096 && !v.chars().any(char::is_control))
}

/// The next size up, or `None` at the ceiling.
pub fn next_tier(size_bytes: u64) -> Option<u64> {
    TIERS_GIB.iter().map(|g| g * GIB).find(|t| *t > size_bytes)
}

#[derive(Debug, PartialEq)]
pub enum Stop {
    Clean,
    Checkpoint,
}

/// §3.2: a box with no user process is stopped and leaves nothing behind; any other is
/// checkpointed. `None` is agentd saying it could not read the cgroups, which is not zero.
pub fn stop_kind(active_user_pids: Option<u64>) -> Stop {
    match active_user_pids {
        Some(0) => Stop::Clean,
        _ => Stop::Checkpoint,
    }
}

/// The two things of a sleeping box the cold tier takes (§3.3): its checkpoint, as `ckpt.tar`,
/// and its disk, as `work.img.zst`.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Part {
    Ckpt,
    Work,
}

impl Part {
    fn name(self) -> &'static str {
        match self {
            Part::Ckpt => "ckpt",
            Part::Work => "work",
        }
    }
}

/// What agentd reports from prepare-sleep (`arbos-agentd/src/sleep.rs`). Only these are read.
#[derive(Deserialize)]
struct SleepReport {
    active_user_pids: Option<u64>,
    files_bytes: u64,
    scratch_bytes: u64,
    trash_bytes: u64,
}

/// A refusal with the status the caller gets. Anything unexpected is a 500.
#[derive(Debug)]
pub struct Error {
    pub status: StatusCode,
    pub msg: String,
}

fn refuse(status: StatusCode, msg: impl Into<String>) -> Error {
    Error {
        status,
        msg: msg.into(),
    }
}

impl<E: Into<anyhow::Error>> From<E> for Error {
    fn from(e: E) -> Self {
        refuse(StatusCode::INTERNAL_SERVER_ERROR, format!("{:#}", e.into()))
    }
}

type Res<T> = Result<T, Error>;

pub struct Cfg {
    /// `/var/lib/arbos-boxd`: `images/` and `boxes/`.
    pub root: String,
    /// runsc's state directory, `/var/run/arbos-runsc`.
    pub run: String,
    /// runsc `--platform`. `systrap` in production; a knob because not every kernel has it.
    pub platform: String,
    /// The first two octets of the /16 the bridge and the boxes take their addresses from. A
    /// knob because the relay dials a box by that address: with more than one host no two may
    /// share a subnet (docs/box-scaling.md Phase 4).
    pub subnet: [u8; 2],
    /// The cold tier's bucket, or `None` while the tier is off.
    pub store: Option<Arc<Store>>,
}

struct Paths {
    /// The sandbox's id in runsc, and the name of its network namespace.
    id: String,
    dir: String,
    img: String,
    mnt: String,
    config: String,
    ckpt: String,
    ip: String,
    log: String,
    /// Where the cold tier keeps this box: `boxes/<user_id>`.
    cold: String,
}

impl Paths {
    /// The part on this host: the checkpoint directory, or the disk image.
    fn local(&self, part: Part) -> &str {
        match part {
            Part::Ckpt => &self.ckpt,
            Part::Work => &self.img,
        }
    }

    fn has(&self, part: Part) -> bool {
        std::fs::symlink_metadata(self.local(part)).is_ok_and(|m| match part {
            Part::Ckpt => m.is_dir(),
            Part::Work => m.is_file(),
        })
    }

    /// The manifest of the part's upload. While it is there the store holds exactly what the
    /// local copy holds, if there is one: it is written before the local copy goes and
    /// removed before the box next runs.
    fn record(&self, part: Part) -> String {
        format!("{}/{}.cold.json", self.dir, part.name())
    }

    /// The packed part on its way to or from the store.
    fn pack(&self, part: Part) -> String {
        format!("{}/{}.pack", self.dir, part.name())
    }

    fn prefix(&self, part: Part) -> String {
        format!("{}/{}", self.cold, part.name())
    }
}

pub struct Boxd {
    cfg: Cfg,
    sys: Arc<dyn Sys>,
    /// One lock per box, so two operations on one box never interleave. An entry stays for
    /// every user this process has seen, which is a few dozen bytes each.
    locks: std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Unpacking an image and handing out an address: one at a time across all boxes. Both are
    /// rare; a first unpack holds other boxes' `ensure` for as long as the tar takes.
    host: tokio::sync::Mutex<()>,
    /// user_id → a prefetch: `None` while it runs, why it failed once it has.
    prefetches: std::sync::Mutex<HashMap<String, Option<String>>>,
}

fn mkdir_private(dir: &str) -> std::io::Result<()> {
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Err(e) if e.kind() != ErrorKind::AlreadyExists => Err(e),
        _ => Ok(()),
    }
}

/// Write a file only root reads, whole or not at all.
fn write_private(path: &str, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = format!("{path}.tmp");
    let mut f = std::fs::File::options()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    std::io::Write::write_all(&mut f, bytes)?;
    std::fs::rename(tmp, path)
}

/// Flush a file, or a directory's entries, to the disk. The cold tier's order of steps (the
/// record before the local copy goes, no record before the box runs) has to hold over a crash.
fn sync(path: &str) -> std::io::Result<()> {
    std::fs::File::open(path)?.sync_all()
}

fn remove_dir_if_there(dir: &str) -> std::io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Bytes on disk of a file, or of the files directly in a directory. A link is not followed.
fn disk_bytes(path: &str) -> u64 {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.blocks() * 512)
            .sum(),
        Ok(m) => m.blocks() * 512,
        Err(_) => 0,
    }
}

/// `MemAvailable` and swap in use, in GiB. The relay's `fleet::parse_meminfo`.
fn parse_meminfo(text: &str) -> (f64, f64) {
    let kb = |key: &str| -> f64 {
        text.lines()
            .find_map(|l| l.strip_prefix(key)?.split_whitespace().next()?.parse().ok())
            .unwrap_or(0.0)
    };
    let gb = |kb: f64| kb / 1024.0 / 1024.0;
    (
        gb(kb("MemAvailable:")),
        gb(kb("SwapTotal:") - kb("SwapFree:")),
    )
}

impl Boxd {
    pub fn new(cfg: Cfg, sys: Arc<dyn Sys>) -> Self {
        Self {
            cfg,
            sys,
            locks: Default::default(),
            host: Default::default(),
            prefetches: Default::default(),
        }
    }

    /// The directories, the two files every box mounts, and the bridge. Safe to run again.
    pub async fn init(&self) -> anyhow::Result<()> {
        let root = &self.cfg.root;
        std::fs::create_dir_all(root)?;
        mkdir_private(&format!("{root}/boxes"))?;
        // A pack exists only mid-archive or mid-fetch: one here at start is a killed daemon's.
        for e in std::fs::read_dir(format!("{root}/boxes"))?.flatten() {
            for pack in ["work.pack", "ckpt.pack"] {
                let _ = std::fs::remove_file(e.path().join(pack));
            }
        }
        mkdir_private(&format!("{root}/images"))?;
        std::fs::write(format!("{root}/resolv.conf"), RESOLV_CONF)?;
        std::fs::write(format!("{root}/hosts"), HOSTS)?;
        let [a, b] = self.cfg.subnet;
        let gateway = format!("{a}.{b}.0.1/16");
        if self.holds(&["ip", "link", "show", BRIDGE]).await? {
            // The bridge and the veth names are one per network namespace: a second daemon
            // here would start, and its boxes would have no gateway.
            let held = self
                .run(&["ip", "-o", "-4", "addr", "show", "dev", BRIDGE])
                .await?;
            if !held.split_whitespace().any(|w| w == gateway) {
                bail!(
                    "the bridge {BRIDGE} is already there without {gateway}: another daemon \
                     on this host has it, and a second one needs its own network namespace"
                );
            }
        } else {
            self.run(&["ip", "link", "add", BRIDGE, "type", "bridge"])
                .await?;
            self.run(&["ip", "addr", "add", &gateway, "dev", BRIDGE])
                .await?;
            self.run(&["ip", "link", "set", BRIDGE, "up"]).await?;
        }
        Ok(())
    }

    /// Run a command that must succeed; its stdout.
    async fn run(&self, argv: &[&str]) -> anyhow::Result<String> {
        self.sys.cmd(argv, None).await?.ok(argv[0])
    }

    /// Run a command that answers with its exit status.
    async fn holds(&self, argv: &[&str]) -> anyhow::Result<bool> {
        Ok(self.sys.cmd(argv, None).await?.ok)
    }

    async fn runsc(&self, args: &[&str], log: Option<&str>) -> anyhow::Result<Out> {
        let root = format!("--root={}", self.cfg.run);
        let platform = format!("--platform={}", self.cfg.platform);
        let mut argv = vec!["runsc", &root, &platform];
        argv.extend(args);
        self.sys.cmd(&argv, log.map(Path::new)).await
    }

    async fn lock(&self, user: &UserId) -> tokio::sync::OwnedMutexGuard<()> {
        let one = self
            .locks
            .lock()
            .unwrap()
            .entry(user.0.clone())
            .or_default()
            .clone();
        one.lock_owned().await
    }

    fn paths(&self, user: &UserId) -> anyhow::Result<Paths> {
        let dir = format!("{}/boxes/{}", self.cfg.root, user.0);
        // `boxes/` is root's alone, so this never fires; it is here so a link there is an
        // error and not a path this daemon formats and mounts as root.
        if std::fs::symlink_metadata(&dir).is_ok_and(|m| !m.is_dir()) {
            bail!("{dir} is not a directory");
        }
        Ok(Paths {
            // runsc takes an id that is the start of exactly one sandbox's as that sandbox, so
            // `u_bob` would name `u_bobby`'s box. No user id holds a dot: the dot ends ours.
            id: format!("arbos-box-{}.", user.0),
            img: format!("{dir}/work.img"),
            mnt: format!("{dir}/mnt"),
            config: format!("{dir}/config.json"),
            ckpt: format!("{dir}/ckpt"),
            ip: format!("{dir}/ip"),
            log: format!("{dir}/box.log"),
            cold: format!("boxes/{}", user.0),
            dir,
        })
    }

    /// A box's `config.json`, or 404 for a box that was never ensured.
    fn config(&self, p: &Paths) -> Res<Value> {
        let bytes = std::fs::read(&p.config)
            .map_err(|_| refuse(StatusCode::NOT_FOUND, "no such box; ensure it first"))?;
        Ok(serde_json::from_slice(&bytes).context("config.json")?)
    }

    fn ip(&self, p: &Paths) -> anyhow::Result<Ipv4Addr> {
        let text = std::fs::read_to_string(&p.ip).context("the box's address")?;
        Ok(text.trim().parse()?)
    }

    async fn running(&self, id: &str) -> anyhow::Result<bool> {
        let o = self.runsc(&["state", id], None).await?;
        Ok(
            o.ok && serde_json::from_str::<Value>(&o.stdout)
                .is_ok_and(|v| v["status"] == "running"),
        )
    }

    /// The rootfs of `version`, unpacked on first use, and what the tar does not carry.
    ///
    /// The operator provides `images/<version>/rootfs.tar` (a `docker export`) and
    /// `image.json` (the image's `Config`); see the crate README.
    async fn image(&self, version: &str) -> Res<(String, oci::Image)> {
        let dir = format!("{}/images/{version}", self.cfg.root);
        let missing = || {
            refuse(
                StatusCode::CONFLICT,
                format!("image {version} is not installed on this host"),
            )
        };
        let meta = std::fs::read(format!("{dir}/image.json")).map_err(|_| missing())?;
        let image = serde_json::from_slice(&meta).context("image.json")?;
        let rootfs = format!("{dir}/rootfs");
        let _host = self.host.lock().await;
        if !Path::new(&rootfs).is_dir() {
            let tar = format!("{dir}/rootfs.tar");
            if !Path::new(&tar).is_file() {
                return Err(missing());
            }
            // Unpacked beside it and renamed, so a half-unpacked tree is never a rootfs.
            let tmp = format!("{dir}/rootfs.tmp");
            remove_dir_if_there(&tmp)?;
            std::fs::create_dir(&tmp)?;
            self.run(&["tar", "-xpf", &tar, "--numeric-owner", "-C", &tmp])
                .await?;
            std::fs::rename(&tmp, &rootfs)?;
            tracing::info!(version, "image unpacked");
        }
        Ok((rootfs, image))
    }

    /// Give the box the lowest free address on the bridge.
    async fn allocate_ip(&self, p: &Paths) -> Res<()> {
        let _host = self.host.lock().await;
        let taken: HashSet<Ipv4Addr> = std::fs::read_dir(format!("{}/boxes", self.cfg.root))?
            .flatten()
            .filter_map(|e| std::fs::read_to_string(e.path().join("ip")).ok())
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        let [a, b] = self.cfg.subnet;
        let ip = (2..=65534u16)
            .map(|n| Ipv4Addr::new(a, b, (n >> 8) as u8, n as u8))
            .find(|ip| !taken.contains(ip))
            .ok_or_else(|| refuse(StatusCode::INSUFFICIENT_STORAGE, "no box address is free"))?;
        Ok(write_private(&p.ip, ip.to_string().as_bytes())?)
    }

    /// `POST /v1/boxes/{user_id}/ensure`: the disk if the box has none, and the bundle.
    pub async fn ensure(
        &self,
        user: &UserId,
        version: &str,
        env: &BTreeMap<String, String>,
    ) -> Res<Value> {
        let _box = self.lock(user).await;
        let p = self.paths(user)?;
        let (rootfs, image) = self.image(version).await?;
        mkdir_private(&p.dir)?;
        mkdir_private(&p.mnt)?;
        // A disk in the cold tier is a disk: no empty one is made in its place.
        let mut cold_work = self.record(&p, Part::Work)?;
        if cold_work.is_none() && !Path::new(&p.img).exists() {
            if let Some(store) = &self.cfg.store {
                // No disk and no record of one here: a new box, or a host that lost its
                // record of an archived disk. The store says which, and a store that cannot
                // say is a refusal, never an empty disk over a user's files.
                let found = store.manifest(&p.prefix(Part::Work)).await.map_err(|e| {
                    let why = format!("the store did not say whether the box has a disk: {e:#}");
                    refuse(StatusCode::BAD_GATEWAY, why)
                })?;
                if let Some(m) = &found {
                    write_private(&p.record(Part::Work), &serde_json::to_vec(m)?)?;
                    sync(&p.record(Part::Work))?;
                    sync(&p.dir)?;
                    tracing::warn!(id = %p.id, "box disk found in the store; its record is back");
                }
                cold_work = found;
            }
        }
        let created = !Path::new(&p.img).exists() && cold_work.is_none();
        if created {
            // Formatted beside it and renamed, so a `work.img` is always a filesystem.
            let tmp = format!("{}.tmp", p.img);
            std::fs::File::create(&tmp)?.set_len(TIERS_GIB[0] * GIB)?;
            // Lazy init is what keeps an empty image at megabytes on disk (§3.3).
            let lazy = "lazy_itable_init=1,lazy_journal_init=1";
            self.run(&[
                "mkfs.ext4",
                "-q",
                "-F",
                "-L",
                "arbos-work",
                "-m",
                "0",
                "-E",
                lazy,
                &tmp,
            ])
            .await?;
            std::fs::rename(&tmp, &p.img)?;
        }
        if !Path::new(&p.ip).exists() {
            self.allocate_ip(&p).await?;
        }
        let root = &self.cfg.root;
        let config = oci::box_config(
            &p.id,
            version,
            &image,
            env,
            &oci::BoxPaths {
                rootfs: &rootfs,
                mnt: &p.mnt,
                resolv: &format!("{root}/resolv.conf"),
                hosts: &format!("{root}/hosts"),
                netns: &format!("/run/netns/{}", p.id),
            },
        );
        let current = std::fs::read(&p.config)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
        if current.as_ref() != Some(&config) {
            // A checkpoint restores only into the bundle it was taken from, and a running
            // box's next checkpoint is taken from this one.
            let checkpointed =
                Path::new(&p.ckpt).exists() || self.record(&p, Part::Ckpt)?.is_some();
            if self.running(&p.id).await? || checkpointed {
                return Err(refuse(
                    StatusCode::CONFLICT,
                    "the box's image or environment changes only while it is asleep with no checkpoint",
                ));
            }
            write_private(&p.config, &serde_json::to_vec_pretty(&config)?)?;
        }
        let size = match cold_work.filter(|_| !p.has(Part::Work)) {
            Some(m) => m.source_bytes,
            None => std::fs::metadata(&p.img)?.len(),
        };
        Ok(json!({ "created": created, "size_gb": size / GIB }))
    }

    /// The part's cold record, if it has one. A record that cannot be read is an error and
    /// not an absence: the box is not started, and no disk is made, on a guess.
    fn record(&self, p: &Paths, part: Part) -> anyhow::Result<Option<Manifest>> {
        match std::fs::read(p.record(part)) {
            Ok(bytes) => {
                Ok(Some(Manifest::parse(&bytes).with_context(|| {
                    format!("{} is not a cold record", p.record(part))
                })?))
            }
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// The part is in the store and not on this host.
    fn cold(&self, p: &Paths, part: Part) -> anyhow::Result<bool> {
        Ok(!p.has(part) && self.record(p, part)?.is_some())
    }

    fn store(&self) -> Res<&Arc<Store>> {
        self.cfg.store.as_ref().ok_or_else(|| {
            refuse(
                StatusCode::CONFLICT,
                "the cold tier is off on this host: ARBOS_BOXD_S3_ENDPOINT is not set",
            )
        })
    }

    /// `POST /v1/boxes/{user_id}/archive`: move a sleeping box's checkpoint or disk to the
    /// object store (§3.3). The local copy goes only after every byte of the upload has been
    /// read back and matched, and after the record of it is on disk.
    pub async fn archive(&self, user: &UserId, part: Part) -> Res<Value> {
        let store = self.store()?;
        let _box = self.lock(user).await;
        let p = self.paths(user)?;
        self.config(&p)?;
        if self.running(&p.id).await? || self.loop_dev(&p.img).await?.is_some() {
            return Err(refuse(
                StatusCode::CONFLICT,
                "the box is awake; only a sleeping box is archived",
            ));
        }
        let known = self.record(&p, part)?;
        let answer =
            |archived: bool, m: &Manifest| json!({ "archived": archived, "stored_bytes": m.bytes });
        if !p.has(part) {
            return match known {
                Some(m) => Ok(answer(false, &m)),
                None => Err(refuse(
                    StatusCode::CONFLICT,
                    format!("the box has no {} to archive", part.name()),
                )),
            };
        }
        let m = match known {
            // Fetched back and not run since: the store still holds exactly this.
            Some(m) => m,
            None => {
                let (local, pack) = (p.local(part), p.pack(part));
                let packed = match part {
                    Part::Ckpt => self.run(&["tar", "-cf", &pack, "-C", local, "."]).await,
                    Part::Work => self.run(&["zstd", "-q", "-f", "-o", &pack, local]).await,
                };
                let source_bytes = match part {
                    Part::Ckpt => 0,
                    Part::Work => std::fs::metadata(local)?.len(),
                };
                let uploaded = match packed {
                    Ok(_) => store.upload(&p.prefix(part), &pack, source_bytes).await,
                    Err(e) => Err(e),
                };
                let _ = std::fs::remove_file(&pack);
                let gateway = |e: anyhow::Error| {
                    refuse(StatusCode::BAD_GATEWAY, format!("archive failed: {e:#}"))
                };
                let m = uploaded.map_err(gateway)?;
                write_private(&p.record(part), &serde_json::to_vec(&m)?)?;
                sync(&p.record(part))?;
                sync(&p.dir)?;
                m
            }
        };
        match part {
            Part::Ckpt => std::fs::remove_dir_all(&p.ckpt)?,
            Part::Work => std::fs::remove_file(&p.img)?,
        }
        tracing::info!(id = %p.id, part = part.name(), stored_bytes = m.bytes, "box part archived");
        Ok(answer(true, &m))
    }

    /// Bring back whatever of the box is in the store, each part checked against its record
    /// before it is given its name. On any failure the box is as cold as it was.
    async fn fetch_cold(&self, p: &Paths) -> Res<()> {
        for part in [Part::Work, Part::Ckpt] {
            let Some(m) = self.record(p, part)?.filter(|_| !p.has(part)) else {
                continue;
            };
            let (local, pack) = (p.local(part), p.pack(part));
            let tmp = format!("{local}.tmp");
            let fetched = async {
                self.store()
                    .map_err(|e| anyhow!(e.msg))?
                    .download(&p.prefix(part), &m, &pack)
                    .await?;
                match part {
                    Part::Ckpt => {
                        remove_dir_if_there(&tmp)?;
                        mkdir_private(&tmp)?;
                        self.run(&["tar", "-xf", &pack, "-C", &tmp]).await?;
                        for file in std::fs::read_dir(&tmp)?.flatten() {
                            std::fs::File::open(file.path())?.sync_all()?;
                        }
                    }
                    Part::Work => {
                        self.run(&["zstd", "-d", "-q", "-f", "--sparse", "-o", &tmp, &pack])
                            .await?;
                        // zstd checks its own frame; this checks it was this disk's.
                        if std::fs::metadata(&tmp)?.len() != m.source_bytes {
                            bail!("the disk that came back is not the size that was archived");
                        }
                    }
                }
                // On the disk before it has its name: a crash leaves the part cold or whole.
                sync(&tmp)?;
                std::fs::rename(&tmp, local)?;
                Ok(sync(&p.dir)?)
            };
            let fetched: anyhow::Result<()> = fetched.await;
            let _ = std::fs::remove_file(&pack);
            fetched.map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                let _ = std::fs::remove_dir_all(&tmp);
                let why = format!(
                    "the box is cold and its {} did not come back: {e:#}",
                    part.name()
                );
                refuse(StatusCode::BAD_GATEWAY, why)
            })?;
            tracing::info!(id = %p.id, part = part.name(), stored_bytes = m.bytes, "box part fetched");
        }
        Ok(())
    }

    /// Drop a part's record, and then what the store holds of it. Called before the local copy
    /// may change or go, which is the moment the store's stops being a copy of it.
    fn forget_cold(&self, p: &Paths, part: Part) -> anyhow::Result<()> {
        let Some(m) = self.record(p, part)? else {
            return Ok(());
        };
        std::fs::remove_file(p.record(part))?;
        sync(&p.dir)?;
        if let Some(store) = self.cfg.store.clone() {
            let (prefix, id) = (p.prefix(part), p.id.clone());
            // Off the wake's clock: a 16 GiB checkpoint is 256 deletes. What a failed one
            // leaves is overwritten by the box's next archive.
            tokio::spawn(async move {
                if let Err(e) = store.remove(&prefix, &m).await {
                    tracing::warn!(%id, error = %format!("{e:#}"), "cold objects not removed");
                }
            });
        }
        Ok(())
    }

    /// `POST /v1/boxes/{user_id}/prefetch`: start bringing a cold box back to this host ahead
    /// of its wake, and answer at once: `cold` is whether it is still on its way. A wake that
    /// comes meanwhile waits on the box's lock and finds the work done. The fetch runs behind
    /// the answer, so a failed one is the next call's to report: it gets the reason as a 502,
    /// once, and the call after that tries again.
    pub fn prefetch(self: &Arc<Self>, user: &UserId) -> Res<Value> {
        let p = self.paths(user)?;
        self.config(&p)?;
        let cold = self.cold(&p, Part::Work)? || self.cold(&p, Part::Ckpt)?;
        if cold {
            self.store()?;
            let mut prefetches = self.prefetches.lock().unwrap();
            match prefetches.get(&user.0) {
                Some(None) => {}
                Some(Some(_)) => {
                    let why = prefetches.remove(&user.0).flatten().unwrap_or_default();
                    return Err(refuse(StatusCode::BAD_GATEWAY, why));
                }
                None => {
                    prefetches.insert(user.0.clone(), None);
                    let (boxd, user) = (self.clone(), user.clone());
                    tokio::spawn(async move {
                        let _box = boxd.lock(&user).await;
                        let fetched = boxd.fetch_cold(&p).await;
                        let mut prefetches = boxd.prefetches.lock().unwrap();
                        match fetched {
                            Ok(()) => drop(prefetches.remove(&user.0)),
                            Err(e) => {
                                tracing::warn!(id = %p.id, error = %e.msg, "prefetch failed");
                                prefetches.insert(user.0, Some(e.msg));
                            }
                        }
                    });
                }
            }
        }
        Ok(json!({ "cold": cold }))
    }

    async fn loop_dev(&self, img: &str) -> anyhow::Result<Option<String>> {
        let out = self.run(&["losetup", "-j", img]).await?;
        let dev = out.split(':').next().unwrap_or("");
        Ok((!dev.is_empty()).then(|| dev.to_owned()))
    }

    /// Attach the image and mount it at `mnt/`, with the two directories the bundle binds.
    async fn mount(&self, p: &Paths) -> anyhow::Result<()> {
        let dev = match self.loop_dev(&p.img).await? {
            Some(dev) => dev,
            None => self.run(&["losetup", "--find", "--show", &p.img]).await?,
        };
        if !self.holds(&["mountpoint", "-q", &p.mnt]).await? {
            self.run(&["mount", "-o", "discard,nosuid,nodev", &dev, &p.mnt])
                .await?;
        }
        let home = format!("{}/.home", p.mnt);
        // The disk is the user's to write. A link at either name would be resolved on the
        // host when runsc binds it, and would hand the box that host directory.
        for dir in [&home, &format!("{}/.etc", p.mnt)] {
            match std::fs::create_dir(dir) {
                Err(e) if e.kind() != ErrorKind::AlreadyExists => return Err(e.into()),
                _ => {}
            }
            if !std::fs::symlink_metadata(dir)?.is_dir() {
                bail!("{dir} is not a directory; refusing to start the box on it");
            }
        }
        self.run(&["chown", "-h", "1000:1000", &p.mnt, &home])
            .await?;
        Ok(())
    }

    /// Bring the box's `/etc` in line with its image: all of it on a first boot, the image's
    /// part whenever the version changed since the last seed.
    async fn seed_etc(&self, p: &Paths, rootfs: &str, version: &str) -> anyhow::Result<()> {
        let etc = format!("{}/.etc", p.mnt);
        let plain = |name: &str| {
            let path = format!("{etc}/{name}");
            std::fs::symlink_metadata(&path)
                .is_ok_and(|m| m.is_file())
                .then_some(path)
        };
        let first_boot = plain("passwd").is_none();
        let seeded = plain(oci::ETC_SEED_MARKER).and_then(|m| std::fs::read_to_string(m).ok());
        if !first_boot && seeded.is_some_and(|v| v.trim() == version) {
            return Ok(());
        }
        let bundle = format!("{}/seed", p.dir);
        mkdir_private(&bundle)?;
        let config = oci::seed_config(rootfs, &etc, first_boot, version);
        write_private(
            &format!("{bundle}/config.json"),
            &serde_json::to_vec(&config)?,
        )?;
        let id = p.id.replacen("box", "seed", 1);
        // What an interrupted seed left behind would refuse the id.
        let _ = self.runsc(&["delete", "--force", &id], None).await;
        self.runsc(&["--network=none", "run", "--bundle", &bundle, &id], None)
            .await?
            .ok("seeding /etc")?;
        std::fs::remove_dir_all(&bundle)?;
        tracing::info!(id = %p.id, version, first_boot, "/etc seeded from the image");
        Ok(())
    }

    /// The host end of a box's veth, named after the last half of its address.
    fn veth(ip: Ipv4Addr) -> String {
        let o = ip.octets();
        format!("ab{:02x}{:02x}", o[2], o[3])
    }

    /// Remove the box's namespace and its veth. Both may already be gone. The veth is removed
    /// by name too: the kernel takes a namespace's devices down some time after the delete.
    async fn net_down(&self, ns: &str, ip: Ipv4Addr) {
        let _ = self.sys.cmd(&["ip", "netns", "del", ns], None).await;
        let _ = self
            .sys
            .cmd(&["ip", "link", "del", &Self::veth(ip)], None)
            .await;
    }

    /// A fresh network namespace for the box: one veth on the bridge, the box's address, and
    /// the bridge as its gateway. runsc reads all three from the namespace.
    async fn net_up(&self, ns: &str, ip: Ipv4Addr) -> anyhow::Result<()> {
        let veth = Self::veth(ip);
        let addr = format!("{ip}/16");
        let [a, b] = self.cfg.subnet;
        let gateway = format!("{a}.{b}.0.1");
        // What a wake that died half way left behind.
        self.net_down(ns, ip).await;
        for argv in [
            &["ip", "netns", "add", ns][..],
            &[
                "ip", "link", "add", &veth, "type", "veth", "peer", "name", "eth0", "netns", ns,
            ],
            &["ip", "link", "set", &veth, "master", BRIDGE, "up"],
            // A new namespace's loopback is down, and runsc gives the sandbox only the
            // interfaces that are up: without it nothing in the box can bind 127.0.0.1.
            &["ip", "-n", ns, "link", "set", "lo", "up"],
            &["ip", "-n", ns, "addr", "add", &addr, "dev", "eth0"],
            &["ip", "-n", ns, "link", "set", "eth0", "up"],
            &["ip", "-n", ns, "route", "add", "default", "via", &gateway],
        ] {
            self.run(argv).await?;
        }
        Ok(())
    }

    /// The second half of a restore: tell agentd its connections are dead, then drop the
    /// checkpoint so the next wake does not restore it again.
    async fn finish_restore(&self, p: &Paths) -> anyhow::Result<()> {
        // agentd is the sandbox's init, which is where `runsc kill` delivers.
        self.runsc(&["kill", &p.id, "SIGUSR1"], None)
            .await?
            .ok("runsc kill SIGUSR1")?;
        Ok(std::fs::remove_dir_all(&p.ckpt)?)
    }

    /// `POST /v1/boxes/{user_id}/wake`: mount the disk and start the box, from its checkpoint
    /// if it has one. A box that is already running is left alone.
    pub async fn wake(&self, user: &UserId) -> Res<Value> {
        let _box = self.lock(user).await;
        let p = self.paths(user)?;
        let config = self.config(&p)?;
        let ip = self.ip(&p)?;
        let has_ckpt = Path::new(&p.ckpt).is_dir();
        let answer = |restored: bool| {
            json!({ "state": "warm", "restored": restored,
                    "box_url": format!("http://{ip}:{AGENTD_PORT}") })
        };
        if self.running(&p.id).await? {
            // An earlier wake restored it and failed before it finished.
            if has_ckpt {
                self.finish_restore(&p).await?;
            }
            return Ok(answer(false));
        }
        // A cold box comes back whole and checked before any of it is mounted or run, and from
        // here on the store's copy is not this box's.
        self.fetch_cold(&p).await?;
        for part in [Part::Work, Part::Ckpt] {
            self.forget_cold(&p, part)?;
        }
        let has_ckpt = Path::new(&p.ckpt).is_dir();
        if !Path::new(&p.img).is_file() {
            return Err(refuse(
                StatusCode::CONFLICT,
                "the box has no disk on this host; ensure it first",
            ));
        }
        // A sandbox that exited by itself still holds its id.
        let _ = self.runsc(&["delete", "--force", &p.id], None).await;
        self.mount(&p).await?;
        let (rootfs, version) = (
            config["root"]["path"].as_str(),
            config["annotations"][oci::VERSION_ANNOTATION].as_str(),
        );
        let (Some(rootfs), Some(version)) = (rootfs, version) else {
            return Err(anyhow!("config.json names no rootfs or version").into());
        };
        self.seed_etc(&p, rootfs, version).await?;
        self.net_up(&p.id, ip).await?;
        // One earlier life of the log is kept.
        let _ = std::fs::rename(&p.log, format!("{}.1", p.log));
        if has_ckpt {
            let image = format!("--image-path={}", p.ckpt);
            self.runsc(
                &["restore", "--bundle", &p.dir, &image, "--detach", &p.id],
                Some(&p.log),
            )
            .await?
            .ok("runsc restore")?;
            self.finish_restore(&p).await?;
        } else {
            self.runsc(
                &["run", "--bundle", &p.dir, "--detach", &p.id],
                Some(&p.log),
            )
            .await?
            .ok("runsc run")?;
        }
        tracing::info!(id = %p.id, restored = has_ckpt, "box awake");
        Ok(answer(has_ckpt))
    }

    /// `POST /v1/boxes/{user_id}/sleep`: ask agentd, then stop the box clean or checkpoint it,
    /// and let go of its disk. 409 while agentd says the box is busy; nothing is stopped then.
    ///
    /// `force` is the user's Stop and an operator's delete: agentd is not asked, a busy box is
    /// stopped like an idle one, and no checkpoint is written or kept, so whatever ran in the
    /// box ends. `/work` stays as it is.
    pub async fn sleep(&self, user: &UserId, force: bool) -> Res<Value> {
        let _box = self.lock(user).await;
        let p = self.paths(user)?;
        let config = self.config(&p)?;
        let mut report = None;
        if self.running(&p.id).await? {
            let kind = if force {
                Stop::Clean
            } else {
                let r = self.prepare_sleep(&p, &config).await?;
                let kind = stop_kind(r.active_user_pids);
                report = Some(r);
                kind
            };
            // One still on disk while the box runs was already restored.
            remove_dir_if_there(&p.ckpt)?;
            match kind {
                Stop::Clean => {
                    // SIGTERM is agentd's flush-and-exit.
                    self.runsc(&["kill", &p.id, "SIGTERM"], None)
                        .await?
                        .ok("runsc kill SIGTERM")?;
                    let wait = ["wait", &p.id];
                    let exited = self.runsc(&wait, None);
                    if tokio::time::timeout(STOP_GRACE, exited).await.is_err() {
                        tracing::warn!(id = %p.id, "box did not exit on SIGTERM; killing it");
                    }
                }
                Stop::Checkpoint => {
                    // Written beside it and renamed, so a `ckpt/` is always a whole checkpoint.
                    let tmp = format!("{}.tmp", p.ckpt);
                    remove_dir_if_there(&tmp)?;
                    let image = format!("--image-path={tmp}");
                    self.runsc(&["checkpoint", &image, &p.id], None)
                        .await?
                        .ok("runsc checkpoint")?;
                    std::fs::rename(&tmp, &p.ckpt)?;
                }
            }
        } else if force {
            self.forget_cold(&p, Part::Ckpt)?;
            remove_dir_if_there(&p.ckpt)?;
        }
        // Also what clears the id of a sandbox that exited by itself.
        let deleted = self.runsc(&["delete", "--force", &p.id], None).await?;
        // Its disk is not taken from under a sandbox runsc still knows, and it is not called
        // asleep: the caller gets the error and the box stays as it is.
        if self.runsc(&["state", &p.id], None).await?.ok {
            return Err(anyhow!(
                "the sandbox is still there after runsc delete: {}",
                deleted.stderr.trim()
            )
            .into());
        }
        if self.holds(&["mountpoint", "-q", &p.mnt]).await? {
            self.run(&["umount", &p.mnt]).await?;
        }
        if let Some(dev) = self.loop_dev(&p.img).await? {
            self.run(&["losetup", "-d", &dev]).await?;
        }
        self.net_down(&p.id, self.ip(&p)?).await;
        let has_ckpt = Path::new(&p.ckpt).is_dir();
        // A box that was not running may have been asleep in the store.
        let cold = self.cold(&p, Part::Work)? || self.cold(&p, Part::Ckpt)?;
        tracing::info!(id = %p.id, checkpointed = has_ckpt, cold, "box asleep");
        let report = report.as_ref();
        let state = match (cold, has_ckpt) {
            (true, _) => "cold",
            (false, true) => "asleep_ckpt",
            (false, false) => "asleep_clean",
        };
        Ok(json!({
            "state": state,
            // The split is agentd's, so it is null when the box was not running to ask.
            "files_bytes": report.map(|r| r.files_bytes),
            "scratch_bytes": report.map(|r| r.scratch_bytes),
            "trash_bytes": report.map(|r| r.trash_bytes),
            "work_bytes": disk_bytes(&p.img),
            "ckpt_bytes": disk_bytes(&p.ckpt),
        }))
    }

    async fn prepare_sleep(&self, p: &Paths, config: &Value) -> Res<SleepReport> {
        // The bearer agentd asks for is the box's MCP token, which the relay put in its env.
        let token = config["process"]["env"]
            .as_array()
            .into_iter()
            .flatten()
            .find_map(|kv| kv.as_str()?.strip_prefix("ARBOS_MCP_TOKEN="))
            .unwrap_or("");
        let gateway = |why: String| refuse(StatusCode::BAD_GATEWAY, why);
        let (status, body) = self
            .sys
            .prepare_sleep(self.ip(p)?, token)
            .await
            .map_err(|e| gateway(format!("agentd did not answer prepare-sleep: {e:#}")))?;
        if status == 409 {
            let reason: String = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| Some(v["reason"].as_str()?.chars().take(200).collect()))
                .unwrap_or_default();
            return Err(refuse(StatusCode::CONFLICT, format!("busy: {reason}")));
        }
        serde_json::from_str(&body)
            .ok()
            .filter(|_| status == 200)
            .ok_or_else(|| gateway(format!("agentd answered prepare-sleep with {status}")))
    }

    /// `POST /v1/boxes/{user_id}/grow`: the next size up, on the mounted disk, without
    /// stopping anything. Refused at 128 GiB.
    pub async fn grow(&self, user: &UserId) -> Res<Value> {
        let _box = self.lock(user).await;
        let p = self.paths(user)?;
        self.config(&p)?;
        let image = std::fs::File::options().write(true).open(&p.img)?;
        let Some(next) = next_tier(image.metadata()?.len()) else {
            let top = TIERS_GIB[TIERS_GIB.len() - 1];
            return Err(refuse(
                StatusCode::CONFLICT,
                format!("the disk is already at its {top} GiB ceiling"),
            ));
        };
        let Some(dev) = self.loop_dev(&p.img).await? else {
            return Err(refuse(
                StatusCode::CONFLICT,
                "the box is asleep; its disk grows while mounted",
            ));
        };
        // If either command fails the file stays a tier ahead of the filesystem. It cannot be
        // cut back under a filesystem that may have grown into it; the next grow moves the
        // file on and resize2fs then takes all of it.
        image.set_len(next)?;
        self.run(&["losetup", "-c", &dev]).await?;
        self.run(&["resize2fs", &dev]).await?;
        tracing::info!(id = %p.id, size_gb = next / GIB, "disk grown");
        Ok(json!({ "size_gb": next / GIB }))
    }

    /// `GET /v1/host/status` (§4.1). Memory, swap and load are null where `/proc` has none.
    /// `cold_tier` is whether this host has an object store; the relay archives only then.
    pub async fn status(&self) -> Res<Value> {
        let listed = self.runsc(&["list", "--quiet"], None).await?;
        let listed = listed.ok("runsc list")?;
        let running: HashSet<&str> = listed.lines().map(str::trim).collect();
        let mut boxes = Vec::new();
        for e in std::fs::read_dir(format!("{}/boxes", self.cfg.root))?.flatten() {
            let Some(user) = e.file_name().to_str().and_then(UserId::parse) else {
                continue;
            };
            // Anything in `boxes/` that is not a box with a disk is not counted.
            let Ok(p) = self.paths(&user) else {
                continue;
            };
            // A record that cannot be read is not a box this can describe.
            let (Ok(work_cold), Ok(ckpt_cold)) =
                (self.cold(&p, Part::Work), self.cold(&p, Part::Ckpt))
            else {
                continue;
            };
            let size = match std::fs::symlink_metadata(&p.img) {
                Ok(image) => image.len(),
                Err(_) if work_cold => self.record(&p, Part::Work)?.map_or(0, |m| m.source_bytes),
                Err(_) => continue,
            };
            boxes.push(json!({
                "user_id": user.0,
                "running": running.contains(p.id.as_str()),
                "size_gb": size / GIB,
                "work_bytes": disk_bytes(&p.img),
                "ckpt_bytes": disk_bytes(&p.ckpt),
                "work_cold": work_cold,
                "ckpt_cold": ckpt_cold,
            }));
        }
        let meminfo = std::fs::read_to_string("/proc/meminfo").ok();
        let (mem, swap) = meminfo.as_deref().map(parse_meminfo).unzip();
        let load5: Option<f64> = std::fs::read_to_string("/proc/loadavg")
            .ok()
            .and_then(|t| t.split_whitespace().nth(1)?.parse().ok());
        let disk = nix::sys::statvfs::statvfs(self.cfg.root.as_str()).context("statvfs")?;
        // Both are u32 on macOS, where this only has to compile and pass its tests.
        #[allow(clippy::unnecessary_cast)]
        let free = disk.blocks_available() as u64 * disk.fragment_size() as u64;
        Ok(json!({
            "mem_available_gb": mem,
            "swap_used_gb": swap,
            "load5": load5,
            "disk_free_gb": free as f64 / 1e9,
            "resident_boxes": boxes.iter().filter(|b| b["running"] == true).count(),
            // A box whose disk is in the store holds none of this host's.
            "present_boxes": boxes.iter().filter(|b| b["work_cold"] == false).count(),
            "cold_tier": self.cfg.store.is_some(),
            "boxes": boxes,
        }))
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::sys::fake::Fake;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A daemon on a fake host, rooted in a fresh temp directory with image `1.0` installed.
    pub async fn boxd(tag: &str) -> (Arc<Boxd>, Arc<Fake>, String) {
        boxd_with(tag, None).await
    }

    async fn boxd_with(tag: &str, store: Option<Store>) -> (Arc<Boxd>, Arc<Fake>, String) {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("arbos-boxd-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let root = root.to_str().unwrap().to_owned();
        let image = format!("{root}/images/1.0");
        std::fs::create_dir_all(&image).unwrap();
        std::fs::write(format!("{image}/rootfs.tar"), b"").unwrap();
        std::fs::write(
            format!("{image}/image.json"),
            r#"{"Entrypoint":["/usr/local/bin/arbos-agentd","boot"],"Cmd":null,
                "Env":["PATH=/usr/bin"],"WorkingDir":"/work"}"#,
        )
        .unwrap();
        let fake = Arc::new(Fake::default());
        let cfg = Cfg {
            run: format!("{root}/run"),
            platform: "systrap".into(),
            root: root.clone(),
            subnet: parse_subnet(SUBNET).unwrap(),
            store: store.map(Arc::new),
        };
        let boxd = Arc::new(Boxd::new(cfg, fake.clone()));
        boxd.init().await.unwrap();
        (boxd, fake, root)
    }

    pub fn user(s: &str) -> UserId {
        UserId::parse(s).unwrap()
    }

    fn env(token: &str) -> BTreeMap<String, String> {
        BTreeMap::from([("ARBOS_MCP_TOKEN".to_owned(), token.to_owned())])
    }

    /// An ensured box, awake.
    async fn awake(tag: &str) -> (Arc<Boxd>, Arc<Fake>, String, UserId) {
        let (b, fake, root) = boxd(tag).await;
        let u = user("01JABCDEFGHJKMNPQRSTVWXYZ0");
        b.ensure(&u, "1.0", &env("t")).await.unwrap();
        b.wake(&u).await.unwrap();
        (b, fake, root, u)
    }

    fn status(r: Res<Value>) -> StatusCode {
        r.unwrap_err().status
    }

    /// Only the relay's id shapes reach a path: nothing with a dot, a slash or a byte outside
    /// ASCII letters, digits, `_` and `-`.
    #[test]
    fn a_user_id_is_one_safe_path_segment() {
        for ok in ["01JABCDEFGHJKMNPQRSTVWXYZ0", "test_01JABC", "u_mac", "a-b"] {
            assert!(UserId::parse(ok).is_some(), "{ok}");
        }
        let long = "a".repeat(65);
        for bad in [
            "", ".", "..", "a/b", "../x", "a.b", "a b", "a\0", "é", &long,
        ] {
            assert!(UserId::parse(bad).is_none(), "{bad:?}");
        }
        assert!(UserId::parse(&"a".repeat(64)).is_some());
    }

    /// runsc resolves an id by prefix, so no box's id may be the start of another's.
    #[tokio::test]
    async fn no_sandbox_id_is_the_start_of_another() {
        let (b, _, root) = boxd("ids").await;
        let id = |u: &str| b.paths(&user(u)).unwrap().id;
        assert_eq!(id("u_bob"), "arbos-box-u_bob.");
        for (a, longer) in [("u_bob", "u_bobby"), ("pfx_1", "pfx_10"), ("a", "a-")] {
            assert!(!id(longer).starts_with(&id(a)), "{a} names {longer}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_version_and_an_environment_are_allowlisted() {
        for ok in ["0.6.409", "1.0-rc_1"] {
            assert!(valid_version(ok), "{ok}");
        }
        for bad in ["", "..", ".hidden", "a/b", "1.0'; rm -rf /", "1 0"] {
            assert!(!valid_version(bad), "{bad:?}");
        }
        let one = |k: &str, v: &str| BTreeMap::from([(k.to_owned(), v.to_owned())]);
        assert!(valid_env(&one(
            "ARBOS_RELAY_URL",
            "https://relay.arbos.life"
        )));
        for (k, v) in [
            ("", "x"),
            ("A=B", "x"),
            ("1A", "x"),
            ("A", "x\ny"),
            ("A B", "x"),
        ] {
            assert!(!valid_env(&one(k, v)), "{k:?}={v:?}");
        }
        assert!(!valid_env(&one("A", &"x".repeat(4097))));
    }

    #[test]
    fn the_disk_grows_through_three_tiers_and_stops() {
        assert_eq!(next_tier(0), Some(8 * GIB));
        // A disk the Docker fleet made is 2 GiB.
        assert_eq!(next_tier(2 * GIB), Some(8 * GIB));
        assert_eq!(next_tier(8 * GIB), Some(32 * GIB));
        assert_eq!(next_tier(32 * GIB), Some(128 * GIB));
        assert_eq!(next_tier(128 * GIB), None);
        assert_eq!(next_tier(129 * GIB), None);
    }

    /// Zero is the only count that stops a box clean. Unknown is not zero.
    #[test]
    fn only_a_box_with_no_user_process_is_stopped_clean() {
        assert_eq!(stop_kind(Some(0)), Stop::Clean);
        assert_eq!(stop_kind(Some(1)), Stop::Checkpoint);
        assert_eq!(stop_kind(None), Stop::Checkpoint);
    }

    #[test]
    fn meminfo_is_read_in_gib() {
        let (mem, swap) =
            parse_meminfo("MemTotal: 9 kB\nMemAvailable: 2097152 kB\nSwapTotal: 3145728 kB\nSwapFree: 1048576 kB\n");
        assert_eq!((mem, swap), (2.0, 2.0));
    }

    /// Ensure makes a sparse 8 GiB image with the lazy-init flags and a 0600 bundle, once.
    #[tokio::test]
    async fn ensure_makes_the_disk_and_the_bundle_once() {
        let (b, fake, root) = boxd("ensure").await;
        let u = user("u_one");
        let first = b.ensure(&u, "1.0", &env("t")).await.unwrap();
        assert_eq!(first, json!({ "created": true, "size_gb": 8 }));
        let dir = format!("{root}/boxes/u_one");
        let image = std::fs::metadata(format!("{dir}/work.img")).unwrap();
        assert_eq!(image.len(), 8 * GIB);
        assert!(image.blocks() * 512 < GIB, "the image is not sparse");
        assert_eq!(
            fake.ran(
                "mkfs.ext4 -q -F -L arbos-work -m 0 -E lazy_itable_init=1,lazy_journal_init=1"
            ),
            1
        );
        let config = std::fs::metadata(format!("{dir}/config.json")).unwrap();
        assert_eq!(config.mode() & 0o777, 0o600, "the bundle holds the tokens");
        assert!(std::path::Path::new(&format!("{root}/images/1.0/rootfs")).is_dir());

        let again = b.ensure(&u, "1.0", &env("t")).await.unwrap();
        assert_eq!(again, json!({ "created": false, "size_gb": 8 }));
        assert_eq!(fake.ran("mkfs.ext4"), 1);
        assert_eq!(fake.ran("tar -xpf"), 1, "the image is unpacked once");

        assert_eq!(
            status(b.ensure(&u, "9.9", &env("t")).await),
            StatusCode::CONFLICT,
            "an image nobody installed"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn every_box_gets_its_own_address() {
        let (b, _fake, root) = boxd("ip").await;
        for u in ["u_a", "u_b", "u_c"] {
            b.ensure(&user(u), "1.0", &env("t")).await.unwrap();
        }
        let ip = |u: &str| std::fs::read_to_string(format!("{root}/boxes/{u}/ip")).unwrap();
        assert_eq!(
            [ip("u_a"), ip("u_b"), ip("u_c")],
            ["10.207.0.2", "10.207.0.3", "10.207.0.4"]
        );
        // A freed address is handed out again, and a box keeps the one it has.
        std::fs::remove_dir_all(format!("{root}/boxes/u_b")).unwrap();
        b.ensure(&user("u_d"), "1.0", &env("t")).await.unwrap();
        b.ensure(&user("u_a"), "1.0", &env("t")).await.unwrap();
        assert_eq!([ip("u_d"), ip("u_a")], ["10.207.0.3", "10.207.0.2"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The bundle of a running or checkpointed box does not change under it: the checkpoint
    /// would not restore. The same ensure again is fine.
    #[tokio::test]
    async fn ensure_does_not_change_a_box_that_is_not_cleanly_asleep() {
        let (b, fake, root, u) = awake("ensure-running").await;
        b.ensure(&u, "1.0", &env("t")).await.unwrap();
        assert_eq!(
            status(b.ensure(&u, "1.0", &env("other")).await),
            StatusCode::CONFLICT
        );
        fake.answer(
            200,
            r#"{"active_user_pids":2,"files_bytes":0,"scratch_bytes":0,"trash_bytes":0}"#,
        );
        b.sleep(&u, false).await.unwrap();
        assert_eq!(
            status(b.ensure(&u, "1.0", &env("other")).await),
            StatusCode::CONFLICT
        );
        b.wake(&u).await.unwrap();
        fake.answer(
            200,
            r#"{"active_user_pids":0,"files_bytes":0,"scratch_bytes":0,"trash_bytes":0}"#,
        );
        b.sleep(&u, false).await.unwrap();
        b.ensure(&u, "1.0", &env("other")).await.unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A first wake attaches, mounts, seeds `/etc`, makes the network and runs the bundle. A
    /// second one finds the box running and touches nothing.
    #[tokio::test]
    async fn wake_starts_a_box_once() {
        let (b, fake, root, u) = awake("wake").await;
        let id = "arbos-box-01JABCDEFGHJKMNPQRSTVWXYZ0.";
        assert_eq!(fake.ran("losetup --find --show"), 1);
        assert_eq!(fake.ran("mount -o discard,nosuid,nodev /dev/loop0"), 1);
        assert_eq!(fake.ran("arbos-seed-01JABCDEFGHJKMNPQRSTVWXYZ0"), 2);
        assert_eq!(fake.ran(&format!("ip netns add {id}")), 1);
        assert_eq!(fake.ran(&format!("ip -n {id} link set lo up")), 1);
        assert_eq!(fake.ran("addr add 10.207.0.2/16 dev eth0"), 1);
        assert_eq!(fake.ran(&format!("--detach {id}")), 1);
        assert_eq!(fake.ran(" restore "), 0);
        assert_eq!(fake.ran("--platform=systrap run --bundle"), 1);

        let before = fake.log.lock().unwrap().len();
        let again = b.wake(&u).await.unwrap();
        assert_eq!(
            again,
            json!({ "state": "warm", "restored": false, "box_url": "http://10.207.0.2:7788" })
        );
        let log = fake.log.lock().unwrap().clone();
        assert_eq!(log.len(), before + 1, "{:?}", &log[before..]);
        assert!(log[before].contains(" state "));

        assert_eq!(
            status(b.wake(&user("u_nobody")).await),
            StatusCode::NOT_FOUND
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A bundle whose disk is gone, with no record of one in the store, is refused in words
    /// before anything is attached.
    #[tokio::test]
    async fn wake_refuses_a_box_with_no_disk() {
        let (b, fake, root) = boxd("no-disk").await;
        let u = user("u_nodisk");
        b.ensure(&u, "1.0", &env("t")).await.unwrap();
        std::fs::remove_file(format!("{root}/boxes/u_nodisk/work.img")).unwrap();
        let e = b.wake(&u).await.unwrap_err();
        assert_eq!(e.status, StatusCode::CONFLICT);
        assert!(e.msg.contains("no disk on this host"), "{}", e.msg);
        assert_eq!((fake.ran("losetup --find"), fake.ran("--detach")), (0, 0));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A second daemon on a host, told another subnet, finds the first one's bridge and does
    /// not start on it.
    #[tokio::test]
    async fn a_bridge_that_is_another_daemons_stops_the_start() {
        let (b, fake, root) = boxd("bridge").await;
        let cfg = Cfg {
            root: root.clone(),
            run: format!("{root}/run"),
            platform: "systrap".into(),
            subnet: parse_subnet("10.208.0.0/16").unwrap(),
            store: None,
        };
        let e = Boxd::new(cfg, fake.clone()).init().await.unwrap_err();
        assert!(e.to_string().contains("without 10.208.0.1/16"), "{e}");
        assert_eq!(fake.ran("ip link add"), 0);
        // The first one starts again on its own.
        b.init().await.unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    /// No user process: SIGTERM, no checkpoint, the disk let go, and the agent's storage
    /// split handed back. A second sleep finds nothing to do.
    #[tokio::test]
    async fn sleep_stops_an_idle_box_clean_and_writes_no_checkpoint() {
        let (b, fake, root, u) = awake("sleep-clean").await;
        fake.answer(
            200,
            r#"{"ok":true,"active_user_pids":0,"files_bytes":5,"scratch_bytes":6,"trash_bytes":7,"vacuumed":[]}"#,
        );
        let r = b.sleep(&u, false).await.unwrap();
        assert_eq!(r["state"], "asleep_clean");
        assert_eq!(
            (&r["files_bytes"], &r["scratch_bytes"], &r["trash_bytes"]),
            (&json!(5), &json!(6), &json!(7))
        );
        assert_eq!(r["ckpt_bytes"], 0);
        assert_eq!(fake.ran("SIGTERM"), 1);
        assert_eq!(fake.ran("checkpoint"), 0);
        assert_eq!(fake.ran("umount"), 1);
        assert_eq!(fake.ran("losetup -d /dev/loop0"), 1);
        assert_eq!(fake.ran("ip netns del"), 2, "one before the wake, one now");
        assert!(!std::path::Path::new(&format!("{root}/boxes/{}/ckpt", u.0)).exists());

        let again = b.sleep(&u, false).await.unwrap();
        assert_eq!(again["state"], "asleep_clean");
        assert_eq!(again["files_bytes"], Value::Null);
        assert_eq!((fake.ran("SIGTERM"), fake.ran("umount")), (1, 1));

        // And it wakes with a plain run.
        b.wake(&u).await.unwrap();
        assert_eq!((fake.ran(" restore "), fake.ran("--detach")), (0, 2));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A sandbox that `runsc delete` did not remove keeps its disk, and the box is not asleep.
    #[tokio::test]
    async fn sleep_fails_while_the_sandbox_is_still_there() {
        let (b, fake, root, u) = awake("sleep-stuck").await;
        fake.answer(
            200,
            r#"{"active_user_pids":0,"files_bytes":0,"scratch_bytes":0,"trash_bytes":0}"#,
        );
        fake.stuck.store(true, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            status(b.sleep(&u, false).await),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!((fake.ran("umount"), fake.ran("losetup -d")), (0, 0));
        fake.stuck
            .store(false, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(b.sleep(&u, false).await.unwrap()["state"], "asleep_clean");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// User processes, or a count agentd could not take: checkpoint, never SIGTERM. The wake
    /// after it restores, signals agentd and drops the checkpoint.
    #[tokio::test]
    async fn sleep_checkpoints_a_box_with_user_processes_and_wake_restores_it() {
        for pids in ["3", "null"] {
            let (b, fake, root, u) = awake("sleep-ckpt").await;
            let ckpt = format!("{root}/boxes/{}/ckpt", u.0);
            fake.answer(
                200,
                &format!(r#"{{"active_user_pids":{pids},"files_bytes":1,"scratch_bytes":1,"trash_bytes":1}}"#),
            );
            let r = b.sleep(&u, false).await.unwrap();
            assert_eq!(r["state"], "asleep_ckpt", "pids {pids}");
            assert_eq!(fake.ran(&format!("checkpoint --image-path={ckpt}.tmp")), 1);
            assert_eq!(fake.ran("SIGTERM"), 0);
            assert_eq!((fake.ran("umount"), fake.ran("losetup -d")), (1, 1));
            assert!(std::path::Path::new(&ckpt).is_dir());

            let woke = b.wake(&u).await.unwrap();
            assert_eq!(woke["restored"], true);
            assert_eq!(
                fake.ran(&format!(
                    "restore --bundle {root}/boxes/{} --image-path={ckpt} --detach",
                    u.0
                )),
                1
            );
            assert_eq!(fake.ran("SIGUSR1"), 1);
            assert!(
                !std::path::Path::new(&ckpt).exists(),
                "restored twice otherwise"
            );
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    /// A busy box answers 409 and stays exactly as it was. So does one whose agentd gave an
    /// answer that cannot be read: nothing is stopped on a guess.
    #[tokio::test]
    async fn sleep_leaves_a_busy_box_running() {
        let (b, fake, root, u) = awake("sleep-busy").await;
        let before = fake.log.lock().unwrap().len();
        fake.answer(409, r#"{"ok":false,"reason":"proj: a turn is running"}"#);
        let e = b.sleep(&u, false).await.unwrap_err();
        assert_eq!(e.status, StatusCode::CONFLICT);
        assert_eq!(e.msg, "busy: proj: a turn is running");
        for (code, body) in [
            (401, ""),
            (200, "not json"),
            (200, r#"{"active_user_pids":0}"#),
        ] {
            fake.answer(code, body);
            assert_eq!(
                status(b.sleep(&u, false).await),
                StatusCode::BAD_GATEWAY,
                "{code} {body}"
            );
        }
        let log = fake.log.lock().unwrap().clone();
        assert!(
            log[before..].iter().all(|l| l.contains(" state ")),
            "{:?}",
            &log[before..]
        );
        assert_eq!(b.wake(&u).await.unwrap()["restored"], false);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A forced sleep stops a box agentd would call busy without asking it, writes no
    /// checkpoint, and drops the one an earlier sleep left: the box can be ensured anew.
    #[tokio::test]
    async fn a_forced_sleep_stops_a_busy_box_and_keeps_no_checkpoint() {
        let (b, fake, root, u) = awake("sleep-force").await;
        let ckpt = format!("{root}/boxes/{}/ckpt", u.0);
        fake.answer(409, r#"{"ok":false,"reason":"proj: a turn is running"}"#);
        let r = b.sleep(&u, true).await.unwrap();
        assert_eq!(r["state"], "asleep_clean");
        assert_eq!(r["files_bytes"], Value::Null, "agentd was not asked");
        assert_eq!((fake.ran("SIGTERM"), fake.ran("checkpoint")), (1, 0));
        assert_eq!((fake.ran("umount"), fake.ran("losetup -d")), (1, 1));

        // Asleep with a checkpoint: forced, the checkpoint goes, and nothing else is run twice.
        b.wake(&u).await.unwrap();
        fake.answer(
            200,
            r#"{"active_user_pids":3,"files_bytes":1,"scratch_bytes":1,"trash_bytes":1}"#,
        );
        assert_eq!(b.sleep(&u, false).await.unwrap()["state"], "asleep_ckpt");
        assert!(std::path::Path::new(&ckpt).is_dir());
        let r = b.sleep(&u, true).await.unwrap();
        assert_eq!(
            (&r["state"], &r["ckpt_bytes"]),
            (&json!("asleep_clean"), &json!(0))
        );
        assert!(!std::path::Path::new(&ckpt).exists());
        assert_eq!((fake.ran("SIGTERM"), fake.ran("checkpoint")), (1, 1));
        assert_eq!(b.wake(&u).await.unwrap()["restored"], false);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Grow needs the disk mounted, walks 8 → 32 → 128 with the loop device told each time,
    /// and refuses past the ceiling.
    #[tokio::test]
    async fn grow_walks_the_tiers_on_a_mounted_disk_and_stops_at_the_ceiling() {
        let (b, fake, root, u) = awake("grow").await;
        let image = format!("{root}/boxes/{}/work.img", u.0);
        let len = || std::fs::metadata(&image).unwrap().len();
        assert_eq!(b.grow(&u).await.unwrap(), json!({ "size_gb": 32 }));
        assert_eq!(len(), 32 * GIB);
        assert_eq!(b.grow(&u).await.unwrap(), json!({ "size_gb": 128 }));
        assert_eq!(len(), 128 * GIB);
        assert_eq!(
            (
                fake.ran("losetup -c /dev/loop0"),
                fake.ran("resize2fs /dev/loop0")
            ),
            (2, 2)
        );
        assert_eq!(status(b.grow(&u).await), StatusCode::CONFLICT);
        assert_eq!((len(), fake.ran("resize2fs")), (128 * GIB, 2));

        let (b, fake, root2) = boxd("grow-asleep").await;
        let u = user("u_asleep");
        b.ensure(&u, "1.0", &env("t")).await.unwrap();
        assert_eq!(status(b.grow(&u).await), StatusCode::CONFLICT);
        assert_eq!(fake.ran("resize2fs"), 0);
        assert_eq!(
            status(b.grow(&user("u_nobody")).await),
            StatusCode::NOT_FOUND
        );
        for r in [root, root2] {
            let _ = std::fs::remove_dir_all(r);
        }
    }

    /// The user's disk can hold a link where the bundle binds a directory. The box is not
    /// started on it.
    #[tokio::test]
    async fn wake_refuses_a_link_where_the_bundle_binds_a_directory() {
        let (b, fake, root) = boxd("link").await;
        let u = user("u_link");
        b.ensure(&u, "1.0", &env("t")).await.unwrap();
        let outside = format!("{root}/outside");
        std::fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, format!("{root}/boxes/u_link/mnt/.etc")).unwrap();
        let e = b.wake(&u).await.unwrap_err();
        assert!(e.msg.contains("is not a directory"), "{}", e.msg);
        assert_eq!(fake.ran("--detach"), 0);
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);

        // And a link standing in for a whole box is not used at all.
        std::os::unix::fs::symlink(&outside, format!("{root}/boxes/u_evil")).unwrap();
        let e = b
            .ensure(&user("u_evil"), "1.0", &env("t"))
            .await
            .unwrap_err();
        assert!(e.msg.contains("is not a directory"), "{}", e.msg);
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn status_counts_present_and_resident_boxes() {
        let (b, _fake, root, u) = awake("status").await;
        b.ensure(&user("u_asleep"), "1.0", &env("t")).await.unwrap();
        let s = b.status().await.unwrap();
        assert_eq!(
            (&s["present_boxes"], &s["resident_boxes"]),
            (&json!(2), &json!(1))
        );
        assert!(s["disk_free_gb"].as_f64().unwrap() > 0.0);
        let boxes = s["boxes"].as_array().unwrap();
        let one = boxes.iter().find(|x| x["user_id"] == u.0.as_str()).unwrap();
        assert_eq!(
            (&one["running"], &one["size_gb"]),
            (&json!(true), &json!(8))
        );
        for key in ["mem_available_gb", "swap_used_gb", "load5"] {
            assert!(s.get(key).is_some(), "{key}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- the cold tier ----

    const CKPT: &str =
        r#"{"active_user_pids":3,"files_bytes":1,"scratch_bytes":1,"trash_bytes":1}"#;

    /// A box asleep with a checkpoint, on a host with an object store.
    async fn asleep_with_a_store(
        tag: &str,
    ) -> (
        Arc<Boxd>,
        Arc<Fake>,
        Arc<crate::cold::tests::Fake>,
        String,
        UserId,
    ) {
        let (bucket, store) = crate::cold::tests::served().await;
        let (b, fake, root) = boxd_with(tag, Some(store)).await;
        let u = user("u_cold");
        b.ensure(&u, "1.0", &env("t")).await.unwrap();
        b.wake(&u).await.unwrap();
        fake.answer(200, CKPT);
        assert_eq!(b.sleep(&u, false).await.unwrap()["state"], "asleep_ckpt");
        (b, fake, bucket, root, u)
    }

    /// A host that lost its record of an archived disk finds it in the store instead of
    /// making an empty one, and a store that cannot say refuses the box.
    #[tokio::test]
    async fn a_lost_record_of_an_archived_disk_comes_back_from_the_store() {
        let (b, fake, bucket, root, u) = asleep_with_a_store("lost-record").await;
        let dir = format!("{root}/boxes/u_cold");
        let (img, record) = (format!("{dir}/work.img"), format!("{dir}/work.cold.json"));
        b.archive(&u, Part::Ckpt).await.unwrap();
        b.archive(&u, Part::Work).await.unwrap();
        let written = std::fs::read(&record).unwrap();
        std::fs::remove_file(&record).unwrap();

        *bucket.fail.lock().unwrap() = Some((403, 99, "work.json"));
        let e = b.ensure(&u, "1.0", &env("t")).await.unwrap_err();
        assert_eq!(e.status, StatusCode::BAD_GATEWAY);
        assert!(!there(&img) && !there(&record));
        *bucket.fail.lock().unwrap() = None;

        assert_eq!(
            b.ensure(&u, "1.0", &env("t")).await.unwrap(),
            json!({ "created": false, "size_gb": 8 })
        );
        assert_eq!(std::fs::read(&record).unwrap(), written);
        assert_eq!(fake.ran("mkfs.ext4"), 1);
        assert_eq!(b.wake(&u).await.unwrap()["restored"], true);
        assert_eq!(std::fs::metadata(&img).unwrap().len(), 8 * GIB);
    }

    /// What happens off the request's clock (a prefetch, the deletes after a wake) happens.
    async fn eventually(what: &str, done: impl Fn() -> bool) {
        for _ in 0..500 {
            if done() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("never: {what}");
    }

    fn there(path: &str) -> bool {
        std::fs::symlink_metadata(path).is_ok()
    }

    /// The checkpoint goes to the store, then the disk; the host keeps a private record of
    /// each and nothing else of them. The box is still a box: ensure makes it no new disk and
    /// status still lists it. Its wake brings both back before anything is mounted, restores
    /// it, and leaves nothing in the store.
    #[tokio::test]
    async fn a_sleeping_box_goes_to_the_store_and_its_wake_brings_it_back() {
        let (b, fake, bucket, root, u) = asleep_with_a_store("cold").await;
        let dir = format!("{root}/boxes/u_cold");
        let (img, ckpt) = (format!("{dir}/work.img"), format!("{dir}/ckpt"));
        let one = |s: &Value| s["boxes"][0].clone();

        let r = b.archive(&u, Part::Ckpt).await.unwrap();
        assert_eq!(r["archived"], true);
        assert!(r["stored_bytes"].as_u64().unwrap() > 0);
        assert!(!there(&ckpt) && there(&img));
        let record = std::fs::metadata(format!("{dir}/ckpt.cold.json")).unwrap();
        assert_eq!(record.mode() & 0o777, 0o600);
        assert!(!there(&format!("{dir}/ckpt.pack")));
        assert!(bucket
            .keys()
            .contains(&"/cold/boxes/u_cold/ckpt/00000".to_owned()));
        let s = b.status().await.unwrap();
        assert_eq!(
            (
                &one(&s)["ckpt_cold"],
                &one(&s)["work_cold"],
                &s["present_boxes"],
                &s["cold_tier"]
            ),
            (&json!(true), &json!(false), &json!(1), &json!(true))
        );
        // Again: nothing to do, and nothing sent.
        let puts = bucket.called("PUT");
        assert_eq!(b.archive(&u, Part::Ckpt).await.unwrap()["archived"], false);
        assert_eq!(bucket.called("PUT"), puts);

        assert_eq!(b.archive(&u, Part::Work).await.unwrap()["archived"], true);
        assert!(!there(&img));
        let s = b.status().await.unwrap();
        assert_eq!(
            (
                &one(&s)["work_cold"],
                &one(&s)["size_gb"],
                &one(&s)["work_bytes"],
                &s["present_boxes"]
            ),
            (&json!(true), &json!(8), &json!(0), &json!(0))
        );
        // A disk in the store is this box's disk, and its checkpoint still pins its bundle.
        assert_eq!(
            b.ensure(&u, "1.0", &env("t")).await.unwrap(),
            json!({ "created": false, "size_gb": 8 })
        );
        assert_eq!(fake.ran("mkfs.ext4"), 1);
        assert_eq!(
            status(b.ensure(&u, "1.0", &env("other")).await),
            StatusCode::CONFLICT
        );
        // An unforced sleep of it stops nothing and says where it is.
        assert_eq!(b.sleep(&u, false).await.unwrap()["state"], "cold");

        let woke = b.wake(&u).await.unwrap();
        assert_eq!(woke["restored"], true);
        assert_eq!(std::fs::metadata(&img).unwrap().len(), 8 * GIB);
        let log = fake.log.lock().unwrap().clone();
        let at = |needle: &str| log.iter().rposition(|l| l.contains(needle)).unwrap();
        assert!(at("zstd -d") < at("mount -o") && at("tar -xf") < at(" restore "));
        for name in [
            "work.cold.json",
            "ckpt.cold.json",
            "work.pack",
            "ckpt.pack",
            "ckpt",
        ] {
            assert!(!there(&format!("{dir}/{name}")), "{name}");
        }
        eventually("the store is emptied", || bucket.keys().is_empty()).await;
        let s = b.status().await.unwrap();
        assert_eq!(
            (
                &one(&s)["work_cold"],
                &one(&s)["ckpt_cold"],
                &s["present_boxes"]
            ),
            (&json!(false), &json!(false), &json!(1))
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A chunk that is not what was uploaded, a store that is down, a record that cannot be
    /// read: the wake fails saying so, nothing is mounted or run, and the box is as cold as it
    /// was. Once the store has the bytes again the same wake goes through.
    #[tokio::test]
    async fn a_cold_box_that_does_not_come_back_whole_is_not_started() {
        let (b, fake, bucket, root, u) = asleep_with_a_store("cold-bad").await;
        let dir = format!("{root}/boxes/u_cold");
        b.archive(&u, Part::Ckpt).await.unwrap();
        b.archive(&u, Part::Work).await.unwrap();
        let before = fake.log.lock().unwrap().len();
        let untouched = |why: &str| {
            let log = fake.log.lock().unwrap().clone();
            let ran = |needle: &str| log[before..].iter().any(|l| l.contains(needle));
            assert!(
                !ran("mount") && !ran("--detach") && !ran("losetup --find"),
                "{why}: {log:?}"
            );
            for gone in [
                "work.img",
                "work.img.tmp",
                "work.pack",
                "ckpt",
                "ckpt.tmp",
                "ckpt.pack",
            ] {
                assert!(!there(&format!("{dir}/{gone}")), "{why}: {gone}");
            }
            for kept in ["work.cold.json", "ckpt.cold.json"] {
                assert!(there(&format!("{dir}/{kept}")), "{why}: {kept}");
            }
        };

        let key = "/cold/boxes/u_cold/work/00000".to_owned();
        let good = bucket.objects.lock().unwrap()[&key].clone();
        let mut bad = good.clone();
        bad[0] ^= 1;
        bucket.objects.lock().unwrap().insert(key.clone(), bad);
        let e = b.wake(&u).await.unwrap_err();
        assert_eq!(e.status, StatusCode::BAD_GATEWAY);
        assert!(e.msg.contains("its work did not come back"), "{}", e.msg);
        untouched("a changed chunk");

        bucket.objects.lock().unwrap().insert(key.clone(), good);
        *bucket.fail.lock().unwrap() = Some((403, 99, "ckpt"));
        let e = b.wake(&u).await.unwrap_err();
        assert!(
            e.msg.contains("its ckpt did not come back") && e.msg.contains("403"),
            "{}",
            e.msg
        );
        assert!(!e.msg.contains("AKIA"), "{}", e.msg);
        // The disk had come back; it stays, and its record with it.
        assert!(there(&format!("{dir}/work.img")) && there(&format!("{dir}/work.cold.json")));
        assert_eq!(fake.ran("mount -o"), 1, "only the first wake mounted");

        *bucket.fail.lock().unwrap() = None;
        let record = format!("{dir}/ckpt.cold.json");
        let kept = std::fs::read(&record).unwrap();
        std::fs::write(&record, b"{}").unwrap();
        assert_eq!(status(b.wake(&u).await), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!((fake.ran("mount -o"), fake.ran(" restore ")), (1, 0));

        std::fs::write(&record, kept).unwrap();
        assert_eq!(b.wake(&u).await.unwrap()["restored"], true);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An upload the store did not keep whole, or refused, costs the box nothing: the local
    /// copy stays, no record is written, and the refusal carries no credential. So do the
    /// archives that make no sense: of a running box, of a checkpoint a box does not have, of a
    /// box nobody made, and any on a host with no store.
    #[tokio::test]
    async fn an_archive_that_did_not_round_trip_keeps_the_local_copy() {
        let (b, fake, bucket, root, u) = asleep_with_a_store("cold-keep").await;
        let dir = format!("{root}/boxes/u_cold");
        let kept = |part: &str| {
            assert!(there(&format!("{dir}/ckpt")) && there(&format!("{dir}/work.img")));
            assert!(!there(&format!("{dir}/{part}.cold.json")));
            assert!(!there(&format!("{dir}/{part}.pack")));
        };
        *bucket.mangle.lock().unwrap() = Some("ckpt/00001");
        let e = b.archive(&u, Part::Ckpt).await.unwrap_err();
        assert_eq!(e.status, StatusCode::BAD_GATEWAY);
        assert!(
            e.msg.contains("not the 4 bytes that were uploaded"),
            "{}",
            e.msg
        );
        kept("ckpt");
        *bucket.mangle.lock().unwrap() = None;
        *bucket.fail.lock().unwrap() = Some((403, 99, "work"));
        let e = b.archive(&u, Part::Work).await.unwrap_err();
        assert!(
            e.msg.contains("403") && !e.msg.contains("AKIA"),
            "{}",
            e.msg
        );
        kept("work");
        *bucket.fail.lock().unwrap() = None;

        b.wake(&u).await.unwrap();
        let puts = bucket.called("PUT");
        assert_eq!(
            status(b.archive(&u, Part::Work).await),
            StatusCode::CONFLICT
        );
        fake.answer(
            200,
            r#"{"active_user_pids":0,"files_bytes":0,"scratch_bytes":0,"trash_bytes":0}"#,
        );
        b.sleep(&u, false).await.unwrap();
        assert_eq!(
            status(b.archive(&u, Part::Ckpt).await),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status(b.archive(&user("u_nobody"), Part::Work).await),
            StatusCode::NOT_FOUND
        );
        assert_eq!(bucket.called("PUT"), puts);

        let (off, _, root2, u2) = awake("cold-off").await;
        assert_eq!(
            status(off.archive(&u2, Part::Work).await),
            StatusCode::CONFLICT
        );
        assert_eq!(off.prefetch(&u2).unwrap(), json!({ "cold": false }));
        assert_eq!(off.status().await.unwrap()["cold_tier"], false);
        for r in [root, root2] {
            let _ = std::fs::remove_dir_all(r);
        }
    }

    /// A prefetch answers at once and brings the box back behind it; the box stays asleep and
    /// its record stays, so archiving it again sends nothing. A wake finds the work done and
    /// fetches nothing twice. A box that is not cold has nothing to prefetch.
    #[tokio::test]
    async fn a_prefetch_stages_a_cold_box_and_the_wake_does_not_fetch_it_again() {
        let (b, fake, bucket, root, u) = asleep_with_a_store("prefetch").await;
        let dir = format!("{root}/boxes/u_cold");
        let (img, ckpt) = (format!("{dir}/work.img"), format!("{dir}/ckpt"));
        assert_eq!(b.prefetch(&u).unwrap(), json!({ "cold": false }));
        b.archive(&u, Part::Ckpt).await.unwrap();
        b.archive(&u, Part::Work).await.unwrap();

        assert_eq!(b.prefetch(&u).unwrap(), json!({ "cold": true }));
        eventually("the box is staged", || there(&img) && there(&ckpt)).await;
        // The prefetch holds the box's lock until it is done.
        drop(b.lock(&u).await);
        assert_eq!(fake.ran("--detach"), 1, "a prefetch starts nothing");
        assert!(there(&format!("{dir}/work.cold.json")) && !bucket.keys().is_empty());

        // Staged and never run: the store's copy is still this box's.
        let puts = bucket.called("PUT");
        assert_eq!(b.archive(&u, Part::Work).await.unwrap()["archived"], true);
        assert!(!there(&img));
        assert_eq!(bucket.called("PUT"), puts);

        // A prefetch that fails says why to the next one, once, and the one after tries again.
        *bucket.fail.lock().unwrap() = Some((403, 99, "work"));
        assert_eq!(b.prefetch(&u).unwrap(), json!({ "cold": true }));
        let failed = || matches!(b.prefetches.lock().unwrap().get("u_cold"), Some(Some(_)));
        eventually("the prefetch fails", failed).await;
        let e = b.prefetch(&u).unwrap_err();
        assert_eq!(e.status, StatusCode::BAD_GATEWAY);
        assert!(
            e.msg.contains("its work did not come back") && e.msg.contains("403"),
            "{}",
            e.msg
        );
        *bucket.fail.lock().unwrap() = None;

        // A prefetch and a wake together: one fetch of the disk between them.
        let gets = bucket.called("GET /cold/boxes/u_cold/work/00000");
        assert_eq!(b.prefetch(&u).unwrap(), json!({ "cold": true }));
        assert_eq!(b.wake(&u).await.unwrap()["restored"], true);
        assert_eq!(bucket.called("GET /cold/boxes/u_cold/work/00000"), gets + 1);
        eventually("the store is emptied", || bucket.keys().is_empty()).await;
        assert_eq!(status(b.prefetch(&user("u_nobody"))), StatusCode::NOT_FOUND);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The user's Stop keeps no checkpoint, in the store either.
    #[tokio::test]
    async fn a_forced_sleep_drops_a_checkpoint_from_the_store_too() {
        let (b, _fake, bucket, root, u) = asleep_with_a_store("cold-force").await;
        b.archive(&u, Part::Ckpt).await.unwrap();
        let r = b.sleep(&u, true).await.unwrap();
        assert_eq!(r["state"], "asleep_clean");
        assert!(!there(&format!("{root}/boxes/u_cold/ckpt.cold.json")));
        eventually("the store is emptied", || bucket.keys().is_empty()).await;
        assert_eq!(b.wake(&u).await.unwrap()["restored"], false);
        b.ensure(&u, "1.0", &env("other")).await.unwrap_err();
        let _ = std::fs::remove_dir_all(&root);
    }
}
