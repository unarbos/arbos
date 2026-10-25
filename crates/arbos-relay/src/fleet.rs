//! Host fleet (D89): user sandboxes as gVisor (`runsc`) containers on the relay host.
//!
//! One always-on sandbox per user, `N` at a time (`ARBOS_HOST_MAX_BOXES`, adjustable at runtime
//! through `PUT /admin/fleet`). A sign-up past `N` — or while the host disk is below its floor —
//! goes on a FIFO waitlist and is admitted when a slot frees. The host evicts only under real
//! resource pressure (available memory, swap, load), least-recently-active first, and never
//! deletes user data: `/work` and `$HOME` live on a per-user sparse ext4 image that stays on
//! disk; an evicted sandbox restarts on the user's next message.
//!
//! The relay drives Docker directly (unit `SupplementaryGroups=docker`). Two things need root
//! on the host and go through a one-shot privileged helper container: attaching and mounting the
//! per-user loop image (Docker's `local` volume driver cannot `-o loop`), and detaching it.
//! Docker's embedded DNS does not work under gVisor on a user-defined network, so every box gets
//! the relay's own `resolv.conf`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use axum::extract::{Path as AxPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use arbos_core::api::{MachineReadyPayloadDto, PushEventDto};
use arbos_core::ids::now_ms;
use arbos_core::wire::BoxState;

use crate::auth::Principal;
use crate::boxes::{self, Provider};
use crate::db::Sandbox;
use crate::signin::api_error;
use crate::state::AppState;

/// Error text `ensure_user_box` returns when the host is full and the user went on the waitlist.
pub const WAITLISTED: &str = "waitlisted: host is full";

/// Fleet knobs (`ARBOS_HOST_*`, `ARBOS_BOX_*`). Defaults are the D89 v1 numbers.
#[derive(Debug, Clone)]
pub struct FleetCfg {
    /// Resident sandboxes the host accepts before sign-ups go on the waitlist.
    pub max_boxes: usize,
    /// Below this much free disk on the relay data volume new sandboxes are not created.
    pub min_free_disk_gb: f64,
    /// Eviction trigger: host `MemAvailable` below this for two consecutive samples.
    pub evict_min_available_gb: f64,
    /// Eviction trigger: swap in use above this for two consecutive samples.
    pub evict_max_swap_gb: f64,
    /// Eviction trigger: 5-minute load average above this.
    pub evict_load: f64,
    pub box_cpus: f64,
    pub box_mem_gb: u64,
    /// Logical size of the per-user `/work` image (sparse on disk).
    pub box_work_gb: u64,
    pub box_pids: u64,
    pub box_network: String,
    pub box_dns: Vec<String>,
    pub helper_image: String,
    pub runtime: String,
    /// Registry path the agent image is pulled from; the tag is the version in the relay's
    /// default snapshot name (`arbos-agent-0.1.55` → `<repo>:0.1.55`), so the CD image roll that
    /// switches `ARBOS_DAYTONA_SNAPSHOT` drives host sandboxes too.
    pub image_repo: String,
}

impl Default for FleetCfg {
    fn default() -> Self {
        Self {
            max_boxes: 80,
            min_free_disk_gb: 40.0,
            evict_min_available_gb: 64.0,
            evict_max_swap_gb: 20.0,
            evict_load: 100.0,
            box_cpus: 2.0,
            // 3 GiB (was 2): a box with four workspaces, a Sonnet helper and Chrome reached the
            // 2 GiB cgroup limit and the host killed the whole sandbox (#1777). 80 × 3 GiB still
            // fits the 256 GiB sandbox budget even if every box sat at its cap.
            box_mem_gb: 3,
            box_work_gb: 2,
            box_pids: 512,
            box_network: "arbos-boxes".into(),
            box_dns: vec!["1.1.1.1".into(), "8.8.8.8".into()],
            helper_image: "ubuntu:24.04".into(),
            runtime: "runsc".into(),
            image_repo: "ghcr.io/unarbos/arbos/arbos-agent".into(),
        }
    }
}

/// The image a sandbox runs for a given snapshot name. `ARBOS_LOCAL_BOX_IMAGE` set to anything
/// but its `arbos-agent:dev` default pins one image (dev relays); otherwise the version in the
/// snapshot name picks the registry tag.
pub fn image_for_snapshot(cfg: &FleetCfg, local_box_image: &str, snapshot: &str) -> String {
    if !local_box_image.is_empty() && local_box_image != "arbos-agent:dev" {
        return local_box_image.to_owned();
    }
    let ver = snapshot
        .rsplit_once('-')
        .map(|(_, v)| v)
        .unwrap_or(snapshot);
    format!("{}:{}", cfg.image_repo, ver)
}

/// Snapshot name the relay's default points at (what `boxes.snapshot` records for every box).
pub fn current_snapshot(state: &AppState) -> String {
    state.default_snapshot()
}

pub fn current_image(state: &AppState) -> String {
    image_for_snapshot(
        &state.cfg.fleet,
        &state.cfg.local_box_image,
        &state.default_snapshot(),
    )
}

impl FleetCfg {
    pub fn from_env() -> Self {
        let d = Self::default();
        let num = |k: &str, dflt: f64| -> f64 {
            std::env::var(k)
                .ok()
                .and_then(|s| s.trim().parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v >= 0.0)
                .unwrap_or(dflt)
        };
        let text = |k: &str, dflt: &str| -> String {
            std::env::var(k)
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| dflt.to_owned())
        };
        Self {
            max_boxes: num("ARBOS_HOST_MAX_BOXES", d.max_boxes as f64) as usize,
            min_free_disk_gb: num("ARBOS_HOST_MIN_FREE_DISK_GB", d.min_free_disk_gb),
            evict_min_available_gb: num(
                "ARBOS_HOST_EVICT_MIN_AVAILABLE_GB",
                d.evict_min_available_gb,
            ),
            evict_max_swap_gb: num("ARBOS_HOST_EVICT_MAX_SWAP_GB", d.evict_max_swap_gb),
            evict_load: num("ARBOS_HOST_EVICT_LOAD", d.evict_load),
            box_cpus: num("ARBOS_BOX_CPUS", d.box_cpus),
            box_mem_gb: num("ARBOS_BOX_MEM_GB", d.box_mem_gb as f64) as u64,
            box_work_gb: num("ARBOS_BOX_WORK_GB", d.box_work_gb as f64) as u64,
            box_pids: num("ARBOS_BOX_PIDS", d.box_pids as f64) as u64,
            box_network: text("ARBOS_BOX_NETWORK", &d.box_network),
            box_dns: text("ARBOS_BOX_DNS", &d.box_dns.join(","))
                .split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect(),
            helper_image: text("ARBOS_BOX_HELPER_IMAGE", &d.helper_image),
            runtime: text("ARBOS_BOX_RUNTIME", &d.runtime),
            image_repo: text("ARBOS_BOX_IMAGE_REPO", &d.image_repo),
        }
    }
}

// ---- host pressure ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Default)]
pub struct Pressure {
    pub at: i64,
    pub mem_available_gb: f64,
    pub swap_used_gb: f64,
    pub load1: f64,
    pub load5: f64,
    pub disk_free_gb: f64,
}

fn parse_meminfo(text: &str) -> (f64, f64) {
    let mut avail = 0.0;
    let mut swap_total = 0.0;
    let mut swap_free = 0.0;
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(k), Some(v)) = (it.next(), it.next()) else {
            continue;
        };
        let kb: f64 = v.parse().unwrap_or(0.0);
        match k {
            "MemAvailable:" => avail = kb,
            "SwapTotal:" => swap_total = kb,
            "SwapFree:" => swap_free = kb,
            _ => {}
        }
    }
    let gb = |kb: f64| kb / 1024.0 / 1024.0;
    (gb(avail), gb(swap_total - swap_free))
}

fn parse_loadavg(text: &str) -> (f64, f64) {
    let mut it = text.split_whitespace();
    let l1 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let l5 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    (l1, l5)
}

async fn disk_free_gb(path: &Path) -> f64 {
    let out = tokio::process::Command::new("df")
        .args(["-B1", "--output=avail"])
        .arg(path)
        .output()
        .await;
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .nth(1)
            .and_then(|l| l.trim().parse::<f64>().ok())
            .map(|b| b / 1e9)
            .unwrap_or(0.0),
        _ => 0.0,
    }
}

pub async fn sample_pressure(state: &AppState) -> Pressure {
    let (mem_available_gb, swap_used_gb) = std::fs::read_to_string("/proc/meminfo")
        .map(|t| parse_meminfo(&t))
        .unwrap_or((f64::MAX, 0.0));
    let (load1, load5) = std::fs::read_to_string("/proc/loadavg")
        .map(|t| parse_loadavg(&t))
        .unwrap_or((0.0, 0.0));
    let disk_free_gb = disk_free_gb(&state.cfg.data_dir).await;
    Pressure {
        at: now_ms(),
        mem_available_gb,
        swap_used_gb,
        load1,
        load5,
        disk_free_gb,
    }
}

/// Which trigger a sample trips, if any. Memory and swap need two consecutive hits (the caller
/// counts them); load uses the 5-minute average so one hit is already sustained.
pub fn eviction_trigger(p: &Pressure, cfg: &FleetCfg, consecutive: u32) -> Option<&'static str> {
    if p.load5 > cfg.evict_load {
        return Some("load");
    }
    if consecutive >= 2 {
        if p.mem_available_gb < cfg.evict_min_available_gb {
            return Some("memory");
        }
        if p.swap_used_gb > cfg.evict_max_swap_gb {
            return Some("swap");
        }
    }
    None
}

/// Does this sample count as a hit at all (for the consecutive counter)?
pub fn pressure_hit(p: &Pressure, cfg: &FleetCfg) -> bool {
    p.mem_available_gb < cfg.evict_min_available_gb
        || p.swap_used_gb > cfg.evict_max_swap_gb
        || p.load5 > cfg.evict_load
}

// ---- capacity gate and waitlist --------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Capacity {
    pub max_boxes: usize,
    pub resident: usize,
    pub waitlist: usize,
    pub disk_free_gb: f64,
    pub min_free_disk_gb: f64,
    /// A new sandbox may be created right now.
    pub has_room: bool,
}

/// `max_boxes` from the DB override (`PUT /admin/fleet`) or the env default.
pub fn max_boxes(state: &AppState) -> usize {
    state
        .db
        .fleet_setting("max_boxes")
        .ok()
        .flatten()
        .and_then(|s| s.parse().ok())
        .unwrap_or(state.cfg.fleet.max_boxes)
}

pub async fn capacity(state: &AppState) -> Capacity {
    let max = max_boxes(state);
    let resident = state
        .db
        .count_boxes_of_provider(Provider::Gvisor.as_str())
        .unwrap_or(usize::MAX);
    let waitlist = state.db.waitlist().map(|w| w.len()).unwrap_or(0);
    let disk_free_gb = disk_free_gb(&state.cfg.data_dir).await;
    Capacity {
        max_boxes: max,
        resident,
        waitlist,
        disk_free_gb,
        min_free_disk_gb: state.cfg.fleet.min_free_disk_gb,
        has_room: room_for_one(
            max,
            resident,
            disk_free_gb,
            state.cfg.fleet.min_free_disk_gb,
        ),
    }
}

/// The gate itself, kept pure for the tests.
pub fn room_for_one(max: usize, resident: usize, disk_free_gb: f64, min_free_gb: f64) -> bool {
    resident < max && disk_free_gb >= min_free_gb
}

/// Admit a user who needs a sandbox: `Ok(true)` = go ahead and create it; `Ok(false)` = the host
/// is full, the user is on the waitlist (idempotent).
pub async fn try_admit(state: &AppState, user_id: &str) -> Result<bool> {
    let cap = capacity(state).await;
    if cap.has_room {
        if state.db.waitlist_remove(user_id)? {
            let _ = state.db.fleet_event("admitted", Some(user_id), "slot free");
        }
        return Ok(true);
    }
    if !state.db.is_waitlisted(user_id)? {
        state.db.waitlist_add(user_id)?;
        let pos = state.db.waitlist_position(user_id)?.unwrap_or(0);
        tracing::info!(%user_id, pos, resident = cap.resident, max = cap.max_boxes, disk_free_gb = cap.disk_free_gb, "host full; user waitlisted");
        let _ = state.db.fleet_event(
            "waitlisted",
            Some(user_id),
            &format!(
                "position {pos}; resident {}/{}; disk {:.0} GB free",
                cap.resident, cap.max_boxes, cap.disk_free_gb
            ),
        );
    }
    Ok(false)
}

// ---- the sandbox itself ----------------------------------------------------------------------

fn container_name(user_id: &str) -> String {
    let safe: String = user_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    format!("arbos-box-{safe}")
}

fn user_dir(state: &AppState, user_id: &str) -> PathBuf {
    state.cfg.data_dir.join("boxes").join(user_id)
}

fn boxes_dir(state: &AppState) -> PathBuf {
    state.cfg.data_dir.join("boxes")
}

/// The `docker run` argument list for one sandbox (everything after `docker`).
pub fn run_args(
    cfg: &FleetCfg,
    name: &str,
    mnt_dir: &Path,
    resolv: &Path,
    image: &str,
    env: &std::collections::BTreeMap<String, String>,
) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "run".into(),
        "-d".into(),
        "--name".into(),
        name.into(),
        format!("--runtime={}", cfg.runtime),
        "--network".into(),
        cfg.box_network.clone(),
        // The image's entrypoint runs as root in Daytona too; in gVisor that root is the Sentry's.
        "-u".into(),
        "0".into(),
        "--read-only".into(),
        "--tmpfs".into(),
        "/tmp:rw,size=512m".into(),
        "--tmpfs".into(),
        "/run:rw,size=16m".into(),
        "--mount".into(),
        format!("type=bind,src={},dst=/work", mnt_dir.display()),
        "--mount".into(),
        format!(
            "type=bind,src={},dst=/home/daytona",
            mnt_dir.join(".home").display()
        ),
        // `/etc` is a writable, persistent copy of the image's `/etc` inside the user's image:
        // agentd's per-project Unix users (`useradd`) must survive restarts, and the rootfs stays
        // read-only.
        "--mount".into(),
        format!("type=bind,src={},dst=/etc", mnt_dir.join(".etc").display()),
        "--shm-size=256m".into(),
        "-v".into(),
        format!("{}:/etc/resolv.conf:ro", resolv.display()),
        format!("--cpus={}", cfg.box_cpus),
        "--cpu-shares=512".into(),
        format!("--memory={}g", cfg.box_mem_gb),
        format!("--memory-swap={}g", cfg.box_mem_gb),
        "--memory-reservation=512m".into(),
        format!("--pids-limit={}", cfg.box_pids),
        "--cap-drop=NET_RAW".into(),
        "--cap-drop=NET_ADMIN".into(),
        "--cap-drop=MKNOD".into(),
        "--cap-drop=AUDIT_WRITE".into(),
        "--security-opt=no-new-privileges".into(),
        "--restart=no".into(),
        "--label".into(),
        "arbos.fleet=gvisor".into(),
        "-p".into(),
        "127.0.0.1::7788".into(),
    ];
    for (k, v) in env {
        a.push("-e".into());
        a.push(format!("{k}={v}"));
    }
    a.push(image.into());
    a
}

async fn docker(args: &[String]) -> Result<String> {
    let out = tokio::process::Command::new("docker")
        .args(args)
        .output()
        .await
        .with_context(|| format!("docker {}", args.first().cloned().unwrap_or_default()))?;
    if !out.status.success() {
        return Err(anyhow!(
            "docker {} failed: {}",
            args.first().cloned().unwrap_or_default(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| (*x).to_owned()).collect()
}

/// Run a root shell on the host through a one-shot privileged helper container with the relay's
/// boxes directory bind-mounted at `/boxes` (shared propagation, so a mount made inside is seen
/// by the host and by Docker). `script` must only use fixed paths under `/boxes`.
async fn helper(state: &AppState, script: &str) -> Result<String> {
    let dir = boxes_dir(state);
    std::fs::create_dir_all(&dir)?;
    docker(&[
        "run".into(),
        "--rm".into(),
        "--privileged".into(),
        "--network".into(),
        "none".into(),
        "--mount".into(),
        format!(
            "type=bind,src={},dst=/boxes,bind-propagation=rshared",
            dir.display()
        ),
        state.cfg.fleet.helper_image.clone(),
        "sh".into(),
        "-c".into(),
        script.into(),
    ])
    .await
}

fn safe_user_segment(user_id: &str) -> Result<()> {
    if user_id.is_empty()
        || user_id.len() > 64
        || !user_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(anyhow!("unsafe user id for a box path"));
    }
    Ok(())
}

/// Make sure the user's `/work` image exists, is attached to a loop device and mounted at
/// `<data>/boxes/<user>/mnt` (idempotent; survives relay and host restarts).
async fn ensure_work_mounted(state: &AppState, user_id: &str) -> Result<PathBuf> {
    safe_user_segment(user_id)?;
    let dir = user_dir(state, user_id);
    let mnt = dir.join("mnt");
    std::fs::create_dir_all(&mnt)?;
    let img = dir.join("work.img");
    if !img.exists() {
        let f = std::fs::File::create(&img)?;
        f.set_len(state.cfg.fleet.box_work_gb * 1024 * 1024 * 1024)?;
        drop(f);
        let out = tokio::process::Command::new("mkfs.ext4")
            .args(["-q", "-F", "-m", "0", "-L", "arbos-work"])
            .arg(&img)
            .output()
            .await
            .context("mkfs.ext4")?;
        if !out.status.success() {
            let _ = std::fs::remove_file(&img);
            return Err(anyhow!(
                "mkfs.ext4 failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
    }
    let script = format!(
        "set -e; img=/boxes/{u}/work.img; mnt=/boxes/{u}/mnt; \
         d=$(losetup -j \"$img\" | cut -d: -f1 | head -1); \
         [ -n \"$d\" ] || d=$(losetup --find --show \"$img\"); \
         mountpoint -q \"$mnt\" || mount -o discard,nosuid,nodev \"$d\" \"$mnt\"; \
         mkdir -p \"$mnt/.home\" \"$mnt/.etc\"; chown 1000:1000 \"$mnt\" \"$mnt/.home\"; echo \"$d\"",
        u = user_id
    );
    let dev = helper(state, &script).await?;
    tracing::debug!(%user_id, dev, "work image mounted");
    Ok(mnt)
}

/// Detach a user's `/work` image (after the container is gone). Never deletes the image.
pub async fn unmount_work(state: &AppState, user_id: &str) -> Result<()> {
    safe_user_segment(user_id)?;
    let script = format!(
        "mnt=/boxes/{u}/mnt; img=/boxes/{u}/work.img; \
         mountpoint -q \"$mnt\" && umount \"$mnt\"; \
         d=$(losetup -j \"$img\" | cut -d: -f1 | head -1); [ -z \"$d\" ] || losetup -d \"$d\"; echo ok",
        u = user_id
    );
    helper(state, &script).await.map(|_| ())
}

/// Account files the box owns: per-project users and groups (agentd's `useradd`) live here, and
/// they are the reason `/etc` is persisted at all. Everything else under `/etc` belongs to the
/// image and follows it.
const ETC_BOX_OWNED: &[&str] = &[
    "passwd",
    "passwd-",
    "group",
    "group-",
    "shadow",
    "shadow-",
    "gshadow",
    "gshadow-",
    "subuid",
    "subuid-",
    "subgid",
    "subgid-",
    ".pwd.lock",
];

/// Image-owned trees under `/etc` that are replaced exactly (a file removed from the image must
/// go from the box too), not merely overwritten.
const ETC_IMAGE_TREES: &[&str] = &["arbos"];

/// Marker of the image whose `/etc` the persisted copy reflects.
const ETC_SEED_MARKER: &str = ".arbos-seed-image";

/// The shell that refreshes a persisted `/etc` from the image it runs in (`/seed` = `<mnt>/.etc`).
/// First boot (no `passwd` yet): the whole `/etc`. Later: everything except the box-owned
/// account files, with the image-owned trees replaced exactly (#1708: the pi-ext seed under
/// `/etc/arbos` stayed at the first image's version for the life of the box).
pub fn etc_refresh_script(first_boot: bool, image: &str) -> String {
    // The marker is written here, by the seed container's root: the persisted /etc is
    // root-owned and the relay process (user `arbos`) cannot write into it (05:10 UTC outage:
    // every recreate died on that write after the old container was already gone).
    let mark = format!(
        "printf '%s\\n' '{}' > /seed/{ETC_SEED_MARKER}",
        image.replace('\'', "")
    );
    if first_boot {
        return format!("set -e; cp -a /etc/. /seed/; {mark}");
    }
    let keep: Vec<String> = ETC_BOX_OWNED
        .iter()
        .map(|f| format!("[ -e /seed/{f} ] && cp -a /seed/{f} /tmp/keep/{f} || true"))
        .collect();
    let restore: Vec<String> = ETC_BOX_OWNED
        .iter()
        .map(|f| format!("[ -e /tmp/keep/{f} ] && cp -a /tmp/keep/{f} /seed/{f} || true"))
        .collect();
    let trees: Vec<String> = ETC_IMAGE_TREES
        .iter()
        .map(|t| format!("rm -rf /seed/{t}; [ -e /etc/{t} ] && cp -a /etc/{t} /seed/{t} || true"))
        .collect();
    format!(
        "set -e; mkdir -p /tmp/keep; {keep}; {trees}; cp -a /etc/. /seed/; {restore}; {mark}",
        keep = keep.join("; "),
        trees = trees.join("; "),
        restore = restore.join("; "),
    )
}

/// Bring the persisted `/etc` of a user's image in line with `image`: full copy on first boot,
/// image-owned parts refreshed whenever the image changed since the last seed. Account files
/// (per-project users) survive.
async fn seed_etc(mnt: &Path, image: &str) -> Result<()> {
    let etc = mnt.join(".etc");
    let first_boot = !etc.join("passwd").exists();
    let marker = etc.join(ETC_SEED_MARKER);
    if !first_boot
        && std::fs::read_to_string(&marker)
            .map(|s| s.trim() == image)
            .unwrap_or(false)
    {
        return Ok(());
    }
    // From inside the image itself: `docker cp` refuses /etc's relative symlinks.
    docker(&[
        "run".into(),
        "--rm".into(),
        "-u".into(),
        "0".into(),
        "--network".into(),
        "none".into(),
        "--mount".into(),
        format!("type=bind,src={},dst=/seed", etc.display()),
        "--entrypoint".into(),
        "sh".into(),
        image.into(),
        "-c".into(),
        etc_refresh_script(first_boot, image),
    ])
    .await?;
    tracing::info!(image, first_boot, mnt = %mnt.display(), "sandbox /etc seeded from the image");
    Ok(())
}

fn resolv_path(state: &AppState) -> Result<PathBuf> {
    let dir = boxes_dir(state);
    std::fs::create_dir_all(&dir)?;
    let p = dir.join("resolv.conf");
    let want = format!(
        "{}\noptions timeout:2 attempts:2\n",
        state
            .cfg
            .fleet
            .box_dns
            .iter()
            .map(|d| format!("nameserver {d}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    if std::fs::read_to_string(&p).ok().as_deref() != Some(&want) {
        std::fs::write(&p, want)?;
    }
    Ok(p)
}

async fn container_exists(name: &str) -> bool {
    docker(&s(&["inspect", "--format", "{{.State.Status}}", name]))
        .await
        .is_ok()
}

async fn record_port(sb: &mut Sandbox, name: &str) -> Result<()> {
    let line = docker(&s(&["port", name, "7788/tcp"])).await?;
    let port = line
        .lines()
        .next()
        .unwrap_or("")
        .rsplit(':')
        .next()
        .unwrap_or("7788")
        .to_owned();
    sb.box_url = Some(format!("http://127.0.0.1:{port}"));
    sb.last_started_at = Some(now_ms());
    Ok(())
}

/// Create the user's sandbox: image + loop mount, then `docker run`.
pub async fn create_box(
    state: &AppState,
    user_id: &str,
    sb: &mut Sandbox,
    box_token: &str,
    mcp_token: &str,
) -> Result<()> {
    let snapshot = sb
        .snapshot
        .clone()
        .unwrap_or_else(|| current_snapshot(state));
    let image = image_for_snapshot(&state.cfg.fleet, &state.cfg.local_box_image, &snapshot);
    let name = container_name(user_id);
    sb.sandbox_id = Some(name.clone());
    sb.device_id = format!("box:{name}");
    sb.snapshot = Some(snapshot);
    state.db.upsert_box(sb)?;
    boxes::save_local_tokens(state, user_id, box_token, mcp_token)?;
    let mnt = ensure_work_mounted(state, user_id).await?;
    seed_etc(&mnt, &image).await?;
    let resolv = resolv_path(state)?;
    if container_exists(&name).await {
        // A stale container from an earlier life (relay DB reset, failed create): replace it.
        let _ = docker(&s(&["rm", "-f", &name])).await;
    }
    let env = boxes::box_env(state, user_id, box_token, mcp_token, &state.cfg.public_url);
    let args = run_args(&state.cfg.fleet, &name, &mnt, &resolv, &image, &env);
    docker(&args).await?;
    record_port(sb, &name).await?;
    state.db.upsert_box(sb)?;
    let _ = state
        .db
        .fleet_event("created", Some(user_id), &format!("{name} ({image})"));
    uid_self_check(state, user_id, &name).await;
    Ok(())
}

/// A file the sandbox's root writes on `/work` must land as uid 0 on the image. Once (2026-10-21)
/// a sandbox wrote everything as the relay's own host uid, which broke the per-project users at
/// the next recreate; this check turns a repeat into a loud event instead of a silent one.
async fn uid_self_check(state: &AppState, user_id: &str, name: &str) {
    let probe =
        "p=/work/.arbos-uidcheck; rm -f \"$p\"; : > \"$p\"; stat -c %u:%g \"$p\"; rm -f \"$p\"";
    match exec_in_box(name, probe, Duration::from_secs(20)).await {
        Ok(out) if out.trim() == "0:0" => {}
        Ok(out) => {
            tracing::error!(%user_id, name, got = %out.trim(), "sandbox uid mapping is wrong: root writes do not land as 0:0");
            let _ = state.db.fleet_event(
                "uid_mapping_wrong",
                Some(user_id),
                &format!("root wrote {} on /work", out.trim()),
            );
        }
        Err(e) => tracing::debug!(%user_id, error = %e, "uid self-check skipped"),
    }
}

/// Image roll for one sandbox: stop and remove the container, start a new one from `snapshot`
/// on the same `/work` image (files, agent memory, per-project users all stay). Seconds, no copy.
pub async fn recreate_with_snapshot(state: &AppState, user_id: &str, snapshot: &str) -> Result<()> {
    let mut sb = state
        .db
        .user_box(user_id)?
        .ok_or_else(|| anyhow!("no box for user {user_id}"))?;
    let (bt, mt) =
        boxes::load_local_tokens(state, user_id)?.ok_or_else(|| anyhow!("local tokens missing"))?;
    let name = sb
        .sandbox_id
        .clone()
        .unwrap_or_else(|| container_name(user_id));
    let from = sb.snapshot.clone();
    if container_exists(&name).await {
        let _ = stop_box(&name, 30).await;
        let _ = remove_container(&name).await;
    }
    sb.snapshot = Some(snapshot.to_owned());
    create_box(state, user_id, &mut sb, &bt, &mt).await?;
    let _ = state.db.fleet_event(
        "upgraded",
        Some(user_id),
        &format!("{} → {snapshot}", from.unwrap_or_default()),
    );
    Ok(())
}

/// Start (or recreate) an existing sandbox after a stop, an eviction, or a host reboot.
pub async fn start_box(
    state: &AppState,
    user_id: &str,
    sb: &mut Sandbox,
    tokens: Option<(String, String)>,
) -> Result<()> {
    let name = sb
        .sandbox_id
        .clone()
        .unwrap_or_else(|| container_name(user_id));
    ensure_work_mounted(state, user_id).await?;
    if !container_exists(&name).await {
        let (bt, mt) = tokens.ok_or_else(|| anyhow!("local tokens missing"))?;
        tracing::warn!(%user_id, name, "sandbox container is gone; recreating it on the kept /work image");
        return create_box(state, user_id, sb, &bt, &mt).await;
    }
    docker(&s(&["start", &name])).await?;
    record_port(sb, &name).await?;
    state.db.upsert_box(sb)?;
    Ok(())
}

pub async fn stop_box(name: &str, grace_s: u32) -> Result<()> {
    docker(&s(&["stop", "-t", &grace_s.to_string(), name])).await?;
    Ok(())
}

/// Remove the container (not the user's image).
pub async fn remove_container(name: &str) -> Result<()> {
    if name.is_empty() {
        return Ok(());
    }
    docker(&s(&["rm", "-f", name])).await.map(|_| ())
}

/// Run a shell command inside a user's sandbox (for workspace copies and archives).
pub async fn exec_in_box(name: &str, script: &str, timeout: Duration) -> Result<String> {
    let args = s(&["exec", name, "sh", "-c", script]);
    tokio::time::timeout(timeout, docker(&args))
        .await
        .map_err(|_| anyhow!("docker exec timed out"))?
}

// ---- eviction --------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub user_id: String,
    pub awake: bool,
    pub turn_running: bool,
    pub last_active_ms: i64,
}

/// Least-recently-active awake sandbox that is not in the middle of a turn.
pub fn pick_eviction(cands: &[Candidate]) -> Option<String> {
    cands
        .iter()
        .filter(|c| c.awake && !c.turn_running)
        .min_by_key(|c| (c.last_active_ms, c.user_id.clone()))
        .map(|c| c.user_id.clone())
}

/// A keepalive said a turn is running this recently → the sandbox is busy.
const TURN_FRESH_MS: i64 = 3 * 60_000;

async fn candidates(state: &AppState) -> Vec<Candidate> {
    let boxes = state.db.list_boxes().unwrap_or_default();
    let turns = state.turn_running.lock().await.clone();
    let pushes = state.last_client_push.lock().await.clone();
    let mut out = Vec::new();
    for b in boxes.into_iter().filter(|b| b.provider == "gvisor") {
        let (st, _) = state.box_state(&b.user_id).await;
        let last_active_ms = [
            b.last_seen_at.unwrap_or(0),
            b.last_started_at.unwrap_or(0),
            pushes.get(&b.user_id).copied().unwrap_or(0),
        ]
        .into_iter()
        .max()
        .unwrap_or(0);
        out.push(Candidate {
            turn_running: turns
                .get(&b.user_id)
                .is_some_and(|t| now_ms() - *t < TURN_FRESH_MS),
            user_id: b.user_id,
            awake: st == BoxState::Awake,
            last_active_ms,
        });
    }
    out
}

/// Evict one sandbox: snapshot first when the runtime supports it, then stop. The `/work`
/// image stays; the user's next message restarts the sandbox.
pub async fn evict(state: &AppState, user_id: &str, trigger: &str) -> Result<()> {
    let sb = state
        .db
        .user_box(user_id)?
        .ok_or_else(|| anyhow!("no box"))?;
    let name = sb
        .sandbox_id
        .clone()
        .unwrap_or_else(|| container_name(user_id));
    // `docker checkpoint` needs the experimental daemon flag; without it this is a no-op and the
    // sandbox cold-starts from its disk on the next wake (2–4 s).
    let ckpt = format!("evict-{}", now_ms());
    let snap = docker(&s(&[
        "checkpoint",
        "create",
        "--leave-running",
        &name,
        &ckpt,
    ]))
    .await;
    if let Err(e) = &snap {
        tracing::debug!(%user_id, error = %e, "checkpoint unavailable; stopping without one");
    }
    stop_box(&name, 30).await?;
    state
        .evicted
        .lock()
        .await
        .insert(user_id.to_owned(), now_ms());
    state.box_conns.lock().await.remove(user_id);
    state
        .set_box_state(
            user_id,
            BoxState::Asleep,
            Some(&format!("evicted: host {trigger} pressure")),
        )
        .await;
    let _ = state.db.fleet_event(
        "evicted",
        Some(user_id),
        &format!(
            "{trigger}; checkpoint {}",
            if snap.is_ok() { "kept" } else { "unavailable" }
        ),
    );
    tracing::warn!(%user_id, trigger, "sandbox evicted under host pressure");
    Ok(())
}

/// The user asked for the machine again: it may wake.
pub async fn clear_evicted(state: &AppState, user_id: &str) -> bool {
    state.evicted.lock().await.remove(user_id).is_some()
}

pub async fn is_evicted(state: &AppState, user_id: &str) -> bool {
    state.evicted.lock().await.contains_key(user_id)
}

// ---- background tasks ------------------------------------------------------------------------

/// Every 30 s: sample host pressure; when a trigger trips, evict least-recently-active sandboxes
/// (at most 5 per tick, re-sampling after each) until it clears. Every 60 s: admit the oldest
/// waitlisted user when a slot is free.
pub async fn spawn_fleet_tasks(state: AppState) {
    if !fleet_enabled(&state) {
        return;
    }
    let st = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        let mut hits: u32 = 0;
        loop {
            tick.tick().await;
            let mut p = sample_pressure(&st).await;
            hits = if pressure_hit(&p, &st.cfg.fleet) {
                hits + 1
            } else {
                0
            };
            *st.fleet_pressure.lock().await = Some(p.clone());
            let mut evicted = 0;
            while let Some(trigger) = eviction_trigger(&p, &st.cfg.fleet, hits) {
                if evicted >= 5 {
                    break;
                }
                let Some(victim) = pick_eviction(&candidates(&st).await) else {
                    tracing::warn!(trigger, "host pressure but no evictable sandbox");
                    break;
                };
                if let Err(e) = evict(&st, &victim, trigger).await {
                    tracing::warn!(user_id = %victim, error = %e, "evict failed");
                    break;
                }
                evicted += 1;
                tokio::time::sleep(Duration::from_secs(5)).await;
                p = sample_pressure(&st).await;
                if trigger == "load" {
                    // load5 moves slowly; one eviction per tick is enough for it.
                    break;
                }
            }
        }
    });
    let st2 = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(300));
        let mut last_alert: HashMap<&'static str, i64> = HashMap::new();
        loop {
            tick.tick().await;
            sample_fleet(&st2, &mut last_alert).await;
        }
    });
    let st3 = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(10));
        let mut last_mem: HashMap<String, f64> = HashMap::new();
        let mut n: u32 = 0;
        loop {
            tick.tick().await;
            // Memory per box every 30 s (one `docker stats`), so a death can be read as OOM.
            if n % 3 == 0 {
                let stats = docker_stats().await;
                for b in st3.db.list_boxes().unwrap_or_default() {
                    if let Some(v) = b.sandbox_id.as_ref().and_then(|nm| stats.get(nm)) {
                        if let Some(m) = v.get("MemUsage").and_then(Value::as_str) {
                            last_mem.insert(b.user_id.clone(), parse_mib(m));
                        }
                    }
                }
            }
            n = n.wrapping_add(1);
            watch_containers(&st3, &last_mem).await;
        }
    });
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            let Ok(list) = state.db.waitlist() else {
                continue;
            };
            let Some(first) = list.first() else {
                continue;
            };
            if !capacity(&state).await.has_room {
                continue;
            }
            let user_id = first.user_id.clone();
            let _ = state.db.waitlist_remove(&user_id);
            let _ = state
                .db
                .fleet_event("admitted", Some(&user_id), "slot free (tick)");
            tracing::info!(%user_id, "waitlist: admitting");
            boxes::ensure_awake(&state, &user_id, "waitlist admitted").await;
            let _ = state
                .db
                .push_user_event(&user_id, &machine_ready_push(), None);
        }
    });
}

/// Push payload when a waitlisted user is admitted (tick and admin admit).
pub(crate) fn machine_ready_push() -> PushEventDto {
    PushEventDto::MachineReady(MachineReadyPayloadDto {
        message: "Your Arbos computer is ready.".into(),
    })
}

// ---- container watch (#1777) ------------------------------------------------------------------

/// What `docker inspect` says about one sandbox container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerFate {
    pub running: bool,
    pub exit_code: i64,
    pub oom_killed: bool,
    pub gone: bool,
}

/// The one line the user sees. The host's memory-cgroup OOM killer takes the gVisor sentry
/// (the whole box) with SIGKILL; docker does not always flag `OOMKilled` for it, so a 137 with
/// the last sample near the cap is read as out of memory too.
pub fn death_reason(f: &ContainerFate, last_mem_mib: Option<f64>, cap_gb: u64) -> String {
    let near_cap = last_mem_mib.is_some_and(|m| m >= cap_gb as f64 * 1024.0 * 0.85);
    if f.gone {
        return "the machine's container disappeared".into();
    }
    if f.oom_killed || (f.exit_code == 137 && near_cap) {
        return format!(
            "out of memory (the machine used all of its {cap_gb} GB; close heavy tools or split the work)"
        );
    }
    if f.exit_code == 137 {
        return "the machine was killed (signal 9)".into();
    }
    format!("the machine stopped unexpectedly (exit {})", f.exit_code)
}

async fn inspect_fate(name: &str) -> ContainerFate {
    match docker(&s(&[
        "inspect",
        "--format",
        "{{.State.Status}} {{.State.ExitCode}} {{.State.OOMKilled}}",
        name,
    ]))
    .await
    {
        Ok(out) => {
            let mut it = out.split_whitespace();
            let status = it.next().unwrap_or("");
            ContainerFate {
                running: status == "running",
                exit_code: it.next().and_then(|v| v.parse().ok()).unwrap_or(-1),
                oom_killed: it.next() == Some("true"),
                gone: false,
            }
        }
        Err(_) => ContainerFate {
            running: false,
            exit_code: -1,
            oom_killed: false,
            gone: true,
        },
    }
}

/// Every 10 s: a sandbox the relay believes awake or waking whose container is not running has
/// died. Record it, tell the clients why, and restart it at once — not after the box's half-open
/// sockets finally time out (16 minutes of silence on 2026-10-25, #1777).
async fn watch_containers(state: &AppState, last_mem: &HashMap<String, f64>) {
    let boxes = state.db.list_boxes().unwrap_or_default();
    for b in boxes.into_iter().filter(|b| b.provider == "gvisor") {
        let (st, _) = state.box_state(&b.user_id).await;
        if !matches!(st, BoxState::Awake | BoxState::Waking) {
            continue;
        }
        if state.upgrading.lock().await.contains_key(&b.user_id) {
            continue;
        }
        let Some(name) = b.sandbox_id.clone() else {
            continue;
        };
        let fate = inspect_fate(&name).await;
        if fate.running {
            continue;
        }
        if fate.gone && st == BoxState::Waking {
            // A create in flight has no container yet.
            continue;
        }
        let reason = death_reason(
            &fate,
            last_mem.get(&b.user_id).copied(),
            state.cfg.fleet.box_mem_gb,
        );
        tracing::error!(user_id = %b.user_id, name, exit = fate.exit_code, oom = fate.oom_killed, gone = fate.gone, %reason, "sandbox container died; restarting");
        let _ = state.db.fleet_event(
            "died",
            Some(&b.user_id),
            &format!(
                "{reason}; exit {}, oom_killed {}, last sample {} MiB",
                fate.exit_code,
                fate.oom_killed,
                last_mem
                    .get(&b.user_id)
                    .map(|m| format!("{m:.0}"))
                    .unwrap_or_else(|| "?".into())
            ),
        );
        state.box_conns.lock().await.remove(&b.user_id);
        state.box_http_seen.lock().await.remove(&b.user_id);
        state.turn_running.lock().await.remove(&b.user_id);
        state
            .set_box_state(
                &b.user_id,
                BoxState::Waking,
                Some(&format!("Machine restarted: {reason}")),
            )
            .await;
        let _ = state.db.push_user(
            &b.user_id,
            "machine_restarted",
            &json!({ "message": format!("Your Arbos computer restarted: {reason}.") }),
            None,
        );
        // `ensure_awake` sees Waking and only runs the stale check; go straight to the start.
        let st2 = state.clone();
        let user = b.user_id.clone();
        tokio::spawn(async move {
            if let Err(e) = boxes::start_box(&st2, &user).await {
                tracing::warn!(user_id = %user, error = %e, "restart after container death failed");
                st2.set_box_state(
                    &user,
                    BoxState::Error,
                    Some(&format!("restart failed: {e}")),
                )
                .await;
            }
        });
    }
}

/// The fleet only runs where gvisor is the default provider or at least one gvisor box exists.
pub fn fleet_enabled(state: &AppState) -> bool {
    boxes::default_provider(state) == Provider::Gvisor
        || state
            .db
            .count_boxes_of_provider(Provider::Gvisor.as_str())
            .unwrap_or(0)
            > 0
}

// ---- admin API -------------------------------------------------------------------------------

fn admin_only(principal: &Principal) -> Result<(), Box<Response>> {
    match principal {
        Principal::Admin => Ok(()),
        Principal::Device { .. } | Principal::Box { .. } => Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "admin token required",
        ))),
    }
}

/// "12.34%" → 12.34; "28.73MiB / 2GiB" → 28.73 (MiB).
fn parse_pct(s: &str) -> f64 {
    s.trim().trim_end_matches('%').parse().unwrap_or(0.0)
}

fn parse_mib(s: &str) -> f64 {
    let first = s.split('/').next().unwrap_or("").trim();
    let num: String = first
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let v: f64 = num.parse().unwrap_or(0.0);
    let unit = first[num.len()..].trim();
    match unit {
        "GiB" => v * 1024.0,
        "KiB" => v / 1024.0,
        "B" => v / 1024.0 / 1024.0,
        "GB" => v * 1000.0 * 1000.0 * 1000.0 / 1024.0 / 1024.0,
        "MB" => v * 1000.0 * 1000.0 / 1024.0 / 1024.0,
        "kB" => v * 1000.0 / 1024.0 / 1024.0,
        _ => v,
    }
}

/// Every 5 min: one `fleet_samples` row per running sandbox plus host alerts (disk under the
/// floor + 20 GB, available memory under the eviction floor + 32 GiB) as `fleet_events`, at most
/// one alert per kind per hour. Samples older than 30 days are pruned.
async fn sample_fleet(state: &AppState, last_alert: &mut HashMap<&'static str, i64>) {
    let p = sample_pressure(state).await;
    let stats = docker_stats().await;
    let host = (p.mem_available_gb, p.load5, p.disk_free_gb);
    for b in state.db.list_boxes().unwrap_or_default() {
        if b.provider != "gvisor" {
            continue;
        }
        let Some(name) = &b.sandbox_id else {
            continue;
        };
        let Some(v) = stats.get(name) else {
            continue;
        };
        let cpu = v
            .get("CPUPerc")
            .and_then(Value::as_str)
            .map(parse_pct)
            .unwrap_or(0.0);
        let mem = v
            .get("MemUsage")
            .and_then(Value::as_str)
            .map(parse_mib)
            .unwrap_or(0.0);
        let pids = v
            .get("PIDs")
            .and_then(Value::as_str)
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let _ = state.db.fleet_sample(&b.user_id, cpu, mem, pids, host);
    }
    let cfg = &state.cfg.fleet;
    let mut alerts: Vec<(&'static str, String)> = Vec::new();
    if p.disk_free_gb < cfg.min_free_disk_gb + 20.0 {
        alerts.push((
            "disk_low",
            format!(
                "{:.0} GB free on the relay data volume (sign-ups pause at {:.0} GB)",
                p.disk_free_gb, cfg.min_free_disk_gb
            ),
        ));
    }
    if p.mem_available_gb < cfg.evict_min_available_gb + 32.0 {
        alerts.push((
            "memory_low",
            format!(
                "{:.0} GiB available on the host (eviction starts at {:.0} GiB)",
                p.mem_available_gb, cfg.evict_min_available_gb
            ),
        ));
    }
    let now = now_ms();
    for (kind, detail) in alerts {
        if last_alert.get(kind).is_some_and(|t| now - *t < 3_600_000) {
            continue;
        }
        last_alert.insert(kind, now);
        tracing::warn!(kind, %detail, "host fleet alert");
        let _ = state.db.fleet_event(kind, None, &detail);
    }
    let _ = state.db.prune_fleet_samples(now - 30 * 24 * 3_600_000);
}

async fn docker_stats() -> HashMap<String, Value> {
    let mut out = HashMap::new();
    let Ok(text) = docker(&s(&["stats", "--no-stream", "--format", "{{json .}}"])).await else {
        return out;
    };
    for line in text.lines() {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if let Some(n) = v.get("Name").and_then(Value::as_str) {
                if n.starts_with("arbos-box-") {
                    out.insert(n.to_owned(), v);
                }
            }
        }
    }
    out
}

/// `GET /admin/fleet`: host pressure, capacity, every gvisor sandbox with its live stats, the
/// waitlist and the last 50 events.
pub async fn admin_fleet(State(state): State<AppState>, principal: Principal) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    let pressure = sample_pressure(&state).await;
    let cap = capacity(&state).await;
    let stats = docker_stats().await;
    let evicted = state.evicted.lock().await.clone();
    let mut sandboxes = Vec::new();
    for b in state.db.list_boxes().unwrap_or_default() {
        if b.provider != "gvisor" {
            continue;
        }
        let (st, since) = state.box_state(&b.user_id).await;
        let name = b.sandbox_id.clone().unwrap_or_default();
        let img = user_dir(&state, &b.user_id).join("work.img");
        let disk_used_mb = std::fs::metadata(&img)
            .ok()
            .map(|m| {
                use std::os::unix::fs::MetadataExt;
                m.blocks() * 512 / 1024 / 1024
            })
            .unwrap_or(0);
        sandboxes.push(json!({
            "user_id": b.user_id,
            "container": name,
            "state": st,
            "since": since,
            "evicted_at": evicted.get(&b.user_id),
            "last_seen_at": b.last_seen_at,
            "last_started_at": b.last_started_at,
            "image": b.snapshot,
            "work_img_used_mb": disk_used_mb,
            "stats": stats.get(&name),
        }));
    }
    Json(json!({
        "enabled": fleet_enabled(&state),
        "default_provider": boxes::default_provider(&state).as_str(),
        "pressure": pressure,
        "capacity": cap,
        "thresholds": {
            "evict_min_available_gb": state.cfg.fleet.evict_min_available_gb,
            "evict_max_swap_gb": state.cfg.fleet.evict_max_swap_gb,
            "evict_load": state.cfg.fleet.evict_load,
        },
        "box": {
            "cpus": state.cfg.fleet.box_cpus, "mem_gb": state.cfg.fleet.box_mem_gb,
            "work_gb": state.cfg.fleet.box_work_gb, "pids": state.cfg.fleet.box_pids,
            "runtime": state.cfg.fleet.runtime, "network": state.cfg.fleet.box_network,
            "image": current_image(&state), "snapshot": current_snapshot(&state),
        },
        "sandboxes": sandboxes,
        "samples": {
            "last_24h": state.db.fleet_sample_summary(now_ms() - 24 * 3_600_000).ok(),
            "last_7d": state.db.fleet_sample_summary(now_ms() - 7 * 24 * 3_600_000).ok(),
        },
        "waitlist": state.db.waitlist().unwrap_or_default(),
        "events": state.db.fleet_events(50).unwrap_or_default(),
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct FleetUpdate {
    pub max_boxes: Option<usize>,
}

/// `PUT /admin/fleet {max_boxes}`: change N at runtime (persisted).
pub async fn admin_fleet_update(
    State(state): State<AppState>,
    principal: Principal,
    Json(b): Json<FleetUpdate>,
) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    if let Some(n) = b.max_boxes {
        if let Err(e) = state.db.set_fleet_setting("max_boxes", &n.to_string()) {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
        }
        let _ = state
            .db
            .fleet_event("cap_changed", None, &format!("max_boxes = {n}"));
    }
    Json(json!({ "capacity": capacity(&state).await })).into_response()
}

/// `POST /admin/waitlist/{id}/admit`: let one user through regardless of their place in line
/// (the gate still applies: a full host refuses).
pub async fn admin_waitlist_admit(
    State(state): State<AppState>,
    principal: Principal,
    AxPath(user_id): AxPath<String>,
) -> Response {
    if let Err(r) = admin_only(&principal) {
        return *r;
    }
    if !capacity(&state).await.has_room {
        return api_error(
            StatusCode::CONFLICT,
            "host_full",
            "no free slot on the host",
        );
    }
    let _ = state.db.waitlist_remove(&user_id);
    let _ = state.db.fleet_event("admitted", Some(&user_id), "by admin");
    boxes::ensure_awake(&state, &user_id, "waitlist admitted").await;
    let _ = state
        .db
        .push_user_event(&user_id, &machine_ready_push(), None);
    Json(json!({ "admitted": user_id, "capacity": capacity(&state).await })).into_response()
}

#[cfg(test)]
mod tests {
    //! D89 invariants: the gate, FIFO admission, least-recently-active eviction that never
    //! touches a running turn, and the sandbox caps in the run arguments.

    use super::*;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    #[test]
    fn gate_counts_residents_and_disk() {
        assert!(room_for_one(80, 79, 200.0, 40.0));
        assert!(!room_for_one(80, 80, 200.0, 40.0), "N reached");
        assert!(!room_for_one(80, 10, 39.9, 40.0), "disk floor");
        assert!(room_for_one(80, 10, 40.0, 40.0));
    }

    #[test]
    fn waitlist_is_fifo_and_idempotent() {
        let state = test_state(AuthMode::Github);
        state.db.waitlist_add("u_b").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        state.db.waitlist_add("u_a").unwrap();
        state.db.waitlist_add("u_b").unwrap(); // keeps its place
        let list = state.db.waitlist().unwrap();
        assert_eq!(
            list.iter().map(|e| e.user_id.as_str()).collect::<Vec<_>>(),
            vec!["u_b", "u_a"]
        );
        assert_eq!(state.db.waitlist_position("u_a").unwrap(), Some(2));
        assert!(state.db.waitlist_remove("u_b").unwrap());
        assert_eq!(state.db.waitlist_position("u_a").unwrap(), Some(1));
        assert!(!state.db.is_waitlisted("u_b").unwrap());
    }

    #[test]
    fn eviction_picks_least_recently_active_and_skips_turns() {
        let c = |u: &str, awake: bool, turn: bool, t: i64| Candidate {
            user_id: u.into(),
            awake,
            turn_running: turn,
            last_active_ms: t,
        };
        let cands = vec![
            c("asleep_oldest", false, false, 1),
            c("busy_old", true, true, 2),
            c("idle_old", true, false, 3),
            c("idle_new", true, false, 4),
        ];
        assert_eq!(pick_eviction(&cands).as_deref(), Some("idle_old"));
        assert_eq!(pick_eviction(&cands[..2]), None, "nothing evictable");
    }

    #[test]
    fn triggers_need_sustained_pressure_except_load() {
        let cfg = FleetCfg::default();
        let p = Pressure {
            mem_available_gb: 10.0,
            ..Default::default()
        };
        assert!(pressure_hit(&p, &cfg));
        assert_eq!(
            eviction_trigger(&p, &cfg, 1),
            None,
            "one sample is not enough"
        );
        assert_eq!(eviction_trigger(&p, &cfg, 2), Some("memory"));
        let swap = Pressure {
            mem_available_gb: 400.0,
            swap_used_gb: 30.0,
            ..Default::default()
        };
        assert_eq!(eviction_trigger(&swap, &cfg, 2), Some("swap"));
        let load = Pressure {
            mem_available_gb: 400.0,
            load5: 120.0,
            ..Default::default()
        };
        assert_eq!(eviction_trigger(&load, &cfg, 0), Some("load"));
        let calm = Pressure {
            mem_available_gb: 400.0,
            ..Default::default()
        };
        assert!(!pressure_hit(&calm, &cfg));
    }

    #[test]
    fn run_args_carry_every_cap() {
        let cfg = FleetCfg::default();
        let env = std::collections::BTreeMap::from([("ARBOS_USER_ID".to_owned(), "u".to_owned())]);
        let a = run_args(
            &cfg,
            "arbos-box-u",
            Path::new("/d/boxes/u/mnt"),
            Path::new("/d/boxes/resolv.conf"),
            "img:1",
            &env,
        );
        let joined = a.join(" ");
        for want in [
            "--runtime=runsc",
            "--network arbos-boxes",
            "--read-only",
            "--cpus=2",
            "--memory=3g",
            "--memory-swap=3g",
            "--pids-limit=512",
            "--security-opt=no-new-privileges",
            "--cap-drop=NET_RAW",
            "type=bind,src=/d/boxes/u/mnt,dst=/work",
            "type=bind,src=/d/boxes/u/mnt/.home,dst=/home/daytona",
            "type=bind,src=/d/boxes/u/mnt/.etc,dst=/etc",
            "--shm-size=256m",
            "/d/boxes/resolv.conf:/etc/resolv.conf:ro",
            "-p 127.0.0.1::7788",
            "-e ARBOS_USER_ID=u",
        ] {
            assert!(joined.contains(want), "missing {want} in {joined}");
        }
        assert_eq!(a.last().map(String::as_str), Some("img:1"));
        assert_eq!(container_name("gh_108417131"), "arbos-box-gh_108417131");
        assert_eq!(container_name("Odd/Id"), "arbos-box-odd-id");
    }

    #[test]
    fn image_follows_the_snapshot_version_unless_pinned() {
        let cfg = FleetCfg::default();
        assert_eq!(
            image_for_snapshot(&cfg, "arbos-agent:dev", "arbos-agent-0.1.55"),
            "ghcr.io/unarbos/arbos/arbos-agent:0.1.55"
        );
        assert_eq!(
            image_for_snapshot(&cfg, "", "arbos-agent-0.1.56"),
            "ghcr.io/unarbos/arbos/arbos-agent:0.1.56"
        );
        assert_eq!(
            image_for_snapshot(&cfg, "my-registry/agent:test", "arbos-agent-0.1.56"),
            "my-registry/agent:test",
            "an explicit image pins"
        );
    }

    #[test]
    fn death_reasons_read_oom_from_flag_or_137_near_the_cap() {
        let oom = ContainerFate {
            running: false,
            exit_code: 137,
            oom_killed: true,
            gone: false,
        };
        assert!(death_reason(&oom, None, 3).contains("out of memory"));
        let sentry = ContainerFate {
            running: false,
            exit_code: 137,
            oom_killed: false,
            gone: false,
        };
        assert!(
            death_reason(&sentry, Some(2900.0), 3).contains("out of memory"),
            "137 at 94 % of 3 GiB"
        );
        assert!(death_reason(&sentry, Some(900.0), 3).contains("signal 9"));
        let crash = ContainerFate {
            running: false,
            exit_code: 1,
            oom_killed: false,
            gone: false,
        };
        assert!(death_reason(&crash, None, 3).contains("exit 1"));
        let gone = ContainerFate {
            running: false,
            exit_code: -1,
            oom_killed: false,
            gone: true,
        };
        assert!(death_reason(&gone, None, 3).contains("disappeared"));
    }

    #[test]
    fn docker_stats_fields_parse() {
        assert!((parse_pct("12.34%") - 12.34).abs() < 1e-9);
        assert!((parse_mib("28.73MiB / 2GiB") - 28.73).abs() < 1e-9);
        assert!((parse_mib("1.5GiB / 2GiB") - 1536.0).abs() < 1e-9);
        assert!((parse_mib("512KiB / 2GiB") - 0.5).abs() < 1e-9);
    }

    #[test]
    fn samples_summarise() {
        let state = test_state(AuthMode::Github);
        for (i, mem) in [100.0, 200.0, 300.0, 400.0].iter().enumerate() {
            state
                .db
                .fleet_sample("u", i as f64, *mem, 80 + i as i64, (400.0, 10.0, 200.0))
                .unwrap();
        }
        let s = state.db.fleet_sample_summary(0).unwrap();
        assert_eq!(s.samples, 4);
        assert_eq!(s.sandboxes, 1);
        assert!((s.mem_mib_avg - 250.0).abs() < 1e-9);
        assert!((s.mem_mib_max - 400.0).abs() < 1e-9);
        assert_eq!(s.pids_max, 83);
        assert_eq!(state.db.prune_fleet_samples(now_ms() + 1).unwrap(), 4);
    }

    #[test]
    fn etc_refresh_keeps_accounts_and_replaces_image_trees() {
        let first = etc_refresh_script(true, "img:1");
        assert!(first.starts_with("set -e; cp -a /etc/. /seed/; printf"));
        assert!(
            first.ends_with("'img:1' > /seed/.arbos-seed-image"),
            "{first}"
        );
        let s = etc_refresh_script(false, "img:1");
        assert!(
            s.ends_with("> /seed/.arbos-seed-image"),
            "marker written by the seed container's root, last"
        );
        assert!(s.contains("cp -a /seed/passwd /tmp/keep/passwd"));
        assert!(s.contains("cp -a /tmp/keep/group /seed/group"));
        assert!(s.contains("rm -rf /seed/arbos; [ -e /etc/arbos ] && cp -a /etc/arbos /seed/arbos"));
        assert!(s.contains("cp -a /etc/. /seed/"));
        // order: keep → trees → full copy → restore
        let (k, t, c, r) = (
            s.find("/tmp/keep/passwd").unwrap(),
            s.find("rm -rf /seed/arbos").unwrap(),
            s.find("cp -a /etc/. /seed/").unwrap(),
            s.rfind("cp -a /tmp/keep/passwd").unwrap(),
        );
        assert!(k < t && t < c && c < r);
    }

    #[test]
    fn meminfo_and_loadavg_parse() {
        let (avail, swap) = parse_meminfo(
            "MemTotal: 100 kB\nMemAvailable: 2097152 kB\nSwapTotal: 4194304 kB\nSwapFree: 3145728 kB\n",
        );
        assert!((avail - 2.0).abs() < 1e-9);
        assert!((swap - 1.0).abs() < 1e-9);
        assert_eq!(parse_loadavg("1.5 2.5 3.5 1/100 42"), (1.5, 2.5));
    }
}
