//! What the box can measure about itself, for `GET /box/state` → Settings → Machine (Jacob,
//! 2026-10-26: "available/used disk, CPU usage, memory usage, per-agent breakdown, running
//! agents/jobs, network, warnings near limits").
//!
//! Inside a Daytona sandbox `/proc/meminfo` and `/proc/loadavg` describe the **host** — measured
//! on Jacob's box (image 0.1.150): MemTotal 755 GiB, load 38 on 64 cores — while the sandbox's own
//! cap is in its cgroup v2 root: `memory.max` = 8 GiB, `cpu.max` = `400000 100000` (4 cores). So:
//!
//! - **Memory and CPU of the box** come from the cgroup root (`memory.current` / `memory.max`,
//!   `cpu.stat usage_usec` over `cpu.max` cores, sampled every `TICK`). `/proc` is the fallback
//!   when there is no cgroup cap (a local box, an old kernel) — `source: "proc"` says so.
//! - **Per workspace**: the `arbos-ws/<user>` cgroups `isolation.rs` creates (`memory.current`,
//!   `cpu.stat`, `pids.current`) plus a cached `du -sm /work/<project>` every `DU_EVERY`.
//! - **Disk**: statvfs of the work root, bytes and inodes (#1857: the lt-eval box refused every
//!   write at 87 % full with 0 inodes).
//! - **Network**: `/proc/net/dev` minus `lo` — rate over the last tick and bytes since the UTC
//!   day started (since agentd started, when that is later).
//! - **Load**: `/proc/loadavg` stays in `/box/state` for older readers, flagged `load_scope:
//!   "host"` whenever a cgroup CPU cap is in force so clients stop drawing it as the box's.
//!
//! Everything is best effort: a missing file is a missing field, never an error. The sampler is
//! one task started from `boot()`; `snapshot()` is what `/box/state` reads.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;

use arbos_core::ids::now_ms;

use crate::isolation::Isolation;

/// Sampling period: the clients poll `/machine/debug` every 5 s while the page is open, so a
/// 5 s window is what they see as "now".
pub const TICK: Duration = Duration::from_secs(5);
/// `du` of every workspace is slow on a big tree (one pi package tree is 18 000 files); once
/// every few minutes is plenty for a disk breakdown.
pub const DU_EVERY: Duration = Duration::from_secs(300);
const DU_TIMEOUT: Duration = Duration::from_secs(90);
const CGROUP_ROOT: &str = "/sys/fs/cgroup";
const CGROUP_PARENT: &str = "arbos-ws";
const MIB: u64 = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Cpu {
    /// Cores the box may use (`cpu.max` quota / period; `nproc` when uncapped).
    pub cores: f64,
    /// Share of those cores busy over the last tick, 0–100. Absent until two samples exist.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pct: Option<f64>,
    /// `cgroup` or `proc`.
    pub source: &'static str,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Memory {
    pub total_mb: u64,
    pub used_mb: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_mb: Option<u64>,
    pub source: &'static str,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Disk {
    pub total_mb: u64,
    pub used_mb: u64,
    pub available_mb: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inodes_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inodes_avail: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Pids {
    pub current: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Net {
    /// Bytes per second over the last tick.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rx_bps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_bps: Option<f64>,
    /// Since the UTC day started — or since agentd started, when that is later.
    pub rx_bytes_today: u64,
    pub tx_bytes_today: u64,
    pub rx_bytes_total: u64,
    pub tx_bytes_total: u64,
}

/// One workspace's share, keyed by project id in the snapshot.
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct WorkspaceUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mem_mb: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pids: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_mb: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_sampled_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Snapshot {
    pub sampled_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<Cpu>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<Memory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk: Option<Disk>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pids: Option<Pids>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net: Option<Net>,
    /// `host` when a cgroup CPU cap is in force (then `/proc/loadavg` is the host's), else `box`.
    pub load_scope: &'static str,
    #[serde(skip)]
    pub workspaces: HashMap<String, WorkspaceUsage>,
}

impl Snapshot {
    pub fn workspace(&self, project_id: &str) -> Option<&WorkspaceUsage> {
        self.workspaces.get(project_id)
    }
}

// ---- parsers (pure; tested) -------------------------------------------------------------------

/// `cpu.max`: "400000 100000" → 4 cores; "max 100000" → `None` (uncapped).
pub fn parse_cpu_max(text: &str) -> Option<f64> {
    let mut it = text.split_whitespace();
    let quota: f64 = it.next()?.parse().ok()?;
    let period: f64 = it.next()?.parse().ok()?;
    (period > 0.0).then(|| quota / period)
}

/// `cpu.stat` → `usage_usec`.
pub fn parse_cpu_stat_usage(text: &str) -> Option<u64> {
    text.lines()
        .find_map(|l| l.strip_prefix("usage_usec"))
        .and_then(|v| v.trim().parse().ok())
}

/// `memory.max` / `memory.current` / `pids.*`: a number, or "max" → `None`.
pub fn parse_cgroup_u64(text: &str) -> Option<u64> {
    text.trim().parse().ok()
}

/// CPU share busy, 0–100: µs of CPU used over µs elapsed, spread over the cores.
pub fn cpu_pct(usage_delta_usec: u64, elapsed_usec: u64, cores: f64) -> Option<f64> {
    if elapsed_usec == 0 || cores <= 0.0 {
        return None;
    }
    let pct = usage_delta_usec as f64 / elapsed_usec as f64 / cores * 100.0;
    Some((pct.clamp(0.0, 100.0) * 10.0).round() / 10.0)
}

/// `/proc/stat` first line → (busy jiffies, total jiffies).
pub fn parse_proc_stat(text: &str) -> Option<(u64, u64)> {
    let line = text.lines().find(|l| l.starts_with("cpu "))?;
    let v: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|x| x.parse().ok())
        .collect();
    if v.len() < 4 {
        return None;
    }
    let idle = v[3] + v.get(4).copied().unwrap_or(0);
    let total: u64 = v.iter().sum();
    Some((total - idle, total))
}

/// `/proc/meminfo` → (MemTotal, MemAvailable) in kB.
pub fn parse_meminfo(text: &str) -> Option<(u64, u64)> {
    let field = |k: &str| {
        text.lines()
            .find(|l| l.starts_with(k))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
    };
    Some((field("MemTotal:")?, field("MemAvailable:")?))
}

/// `/proc/net/dev` → (rx bytes, tx bytes) summed over every interface but `lo`.
pub fn parse_net_dev(text: &str) -> (u64, u64) {
    let mut rx = 0;
    let mut tx = 0;
    for line in text.lines().skip(2) {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        if name.trim() == "lo" {
            continue;
        }
        let f: Vec<u64> = rest
            .split_whitespace()
            .filter_map(|x| x.parse().ok())
            .collect();
        if f.len() >= 9 {
            rx += f[0];
            tx += f[8];
        }
    }
    (rx, tx)
}

/// `du -sm <dir>` output → MiB.
pub fn parse_du(text: &str) -> Option<u64> {
    text.split_whitespace().next()?.parse().ok()
}

/// Days since the epoch, UTC — the "today" boundary for the network counters.
pub fn utc_day(now_ms: i64) -> i64 {
    now_ms.div_euclid(86_400_000)
}

// ---- the sampler ------------------------------------------------------------------------------

#[derive(Default)]
struct CpuSample {
    usage_usec: u64,
    at: Option<Instant>,
}

#[derive(Default)]
struct ProcSample {
    busy: u64,
    total: u64,
}

#[derive(Default)]
struct NetBase {
    day: i64,
    rx: u64,
    tx: u64,
}

#[derive(Default)]
struct State {
    snapshot: Snapshot,
    box_cpu: CpuSample,
    proc_cpu: Option<ProcSample>,
    ws_cpu: HashMap<String, CpuSample>,
    net_prev: Option<(Instant, u64, u64)>,
    net_base: Option<NetBase>,
    du: HashMap<String, (u64, i64)>,
    du_at: Option<Instant>,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State::default()))
}

/// The latest sample. Empty (`sampled_at` 0) until the first tick ran.
pub fn snapshot() -> Snapshot {
    state()
        .lock()
        .map(|s| s.snapshot.clone())
        .unwrap_or_default()
}

/// Start the sampler: one tick now, then every `TICK`; `du` of every workspace every `DU_EVERY`.
pub fn start(work_root: PathBuf) {
    tokio::spawn(async move {
        loop {
            let du_due = state()
                .lock()
                .map(|s| s.du_at.is_none_or(|t| t.elapsed() >= DU_EVERY))
                .unwrap_or(false);
            if du_due {
                let sizes = du_workspaces(&work_root).await;
                if let Ok(mut s) = state().lock() {
                    let now = now_ms();
                    s.du = sizes.into_iter().map(|(k, v)| (k, (v, now))).collect();
                    s.du_at = Some(Instant::now());
                }
            }
            let root = work_root.clone();
            let _ = tokio::task::spawn_blocking(move || tick(&root)).await;
            tokio::time::sleep(TICK).await;
        }
    });
}

fn read(p: impl AsRef<Path>) -> Option<String> {
    std::fs::read_to_string(p).ok()
}

/// Workspace project ids: the directories under the work root (`.arbos` is box state).
fn workspace_dirs(work_root: &Path) -> Vec<(String, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(work_root) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            (!name.starts_with('.')).then(|| (name, e.path()))
        })
        .collect();
    out.sort();
    out
}

async fn du_workspaces(work_root: &Path) -> HashMap<String, u64> {
    let mut out = HashMap::new();
    for (id, dir) in workspace_dirs(work_root) {
        let run = tokio::process::Command::new("du")
            .args(["-sm", "-x"])
            .arg(&dir)
            .stderr(std::process::Stdio::null())
            .output();
        match tokio::time::timeout(DU_TIMEOUT, run).await {
            Ok(Ok(o)) => {
                if let Some(mb) = parse_du(&String::from_utf8_lossy(&o.stdout)) {
                    out.insert(id, mb);
                }
            }
            Ok(Err(e)) => tracing::debug!(error = %e, "du"),
            Err(_) => tracing::debug!(%id, "du timed out"),
        }
    }
    out
}

// statvfs field widths differ per libc (u32 on macOS, u64 on Linux).
#[allow(clippy::unnecessary_cast)]
fn disk_of(path: &Path) -> Option<Disk> {
    let s = nix::sys::statvfs::statvfs(path).ok()?;
    let frag = s.fragment_size() as u64;
    let total = s.blocks() as u64 * frag / MIB;
    let avail = s.blocks_available() as u64 * frag / MIB;
    let free = s.blocks_free() as u64 * frag / MIB;
    let files = s.files() as u64;
    Some(Disk {
        total_mb: total,
        used_mb: total.saturating_sub(free),
        available_mb: avail,
        inodes_total: (files > 0).then_some(files),
        inodes_avail: (files > 0).then_some(s.files_available() as u64),
    })
}

fn tick(work_root: &Path) {
    let Ok(mut s) = state().lock() else {
        return;
    };
    let now = Instant::now();
    let now_ms = now_ms();
    let root = Path::new(CGROUP_ROOT);
    let mut snap = Snapshot {
        sampled_at: now_ms,
        load_scope: "box",
        ..Default::default()
    };

    // CPU of the box: the cgroup root when capped, else /proc/stat.
    let cores_capped = read(root.join("cpu.max")).and_then(|t| parse_cpu_max(&t));
    let nproc = std::thread::available_parallelism()
        .map(|n| n.get() as f64)
        .unwrap_or(1.0);
    let usage = read(root.join("cpu.stat")).and_then(|t| parse_cpu_stat_usage(&t));
    match (cores_capped, usage) {
        (Some(cores), Some(usage_usec)) => {
            snap.load_scope = "host";
            let pct = match s.box_cpu.at {
                Some(at) => cpu_pct(
                    usage_usec.saturating_sub(s.box_cpu.usage_usec),
                    now.duration_since(at).as_micros() as u64,
                    cores,
                ),
                None => None,
            };
            s.box_cpu = CpuSample {
                usage_usec,
                at: Some(now),
            };
            snap.cpu = Some(Cpu {
                cores,
                pct,
                source: "cgroup",
            });
        }
        _ => {
            if let Some((busy, total)) = read("/proc/stat").and_then(|t| parse_proc_stat(&t)) {
                let pct = s.proc_cpu.as_ref().and_then(|p| {
                    let dt = total.saturating_sub(p.total);
                    (dt > 0).then(|| {
                        ((busy.saturating_sub(p.busy) as f64 / dt as f64 * 1000.0).round() / 10.0)
                            .clamp(0.0, 100.0)
                    })
                });
                s.proc_cpu = Some(ProcSample { busy, total });
                snap.cpu = Some(Cpu {
                    cores: nproc,
                    pct,
                    source: "proc",
                });
            }
        }
    }

    // Memory of the box: cgroup cap when set, else /proc/meminfo.
    let mem_max = read(root.join("memory.max")).and_then(|t| parse_cgroup_u64(&t));
    let mem_cur = read(root.join("memory.current")).and_then(|t| parse_cgroup_u64(&t));
    snap.memory = match (mem_max, mem_cur) {
        (Some(max), Some(cur)) => Some(Memory {
            total_mb: max / MIB,
            used_mb: cur / MIB,
            peak_mb: read(root.join("memory.peak"))
                .and_then(|t| parse_cgroup_u64(&t))
                .map(|p| p / MIB),
            source: "cgroup",
        }),
        _ => read("/proc/meminfo")
            .and_then(|t| parse_meminfo(&t))
            .map(|(total_kb, avail_kb)| Memory {
                total_mb: total_kb / 1024,
                used_mb: total_kb.saturating_sub(avail_kb) / 1024,
                peak_mb: None,
                source: "proc",
            }),
    };

    snap.disk = disk_of(work_root);
    snap.pids = read(root.join("pids.current"))
        .and_then(|t| parse_cgroup_u64(&t))
        .map(|current| Pids {
            current,
            max: read(root.join("pids.max")).and_then(|t| parse_cgroup_u64(&t)),
        });

    // Network: /proc/net/dev minus lo; rate over the tick, bytes since the UTC day began.
    if let Some(text) = read("/proc/net/dev") {
        let (rx, tx) = parse_net_dev(&text);
        let day = utc_day(now_ms);
        let base = match s.net_base.take() {
            Some(b) if b.day == day && b.rx <= rx && b.tx <= tx => b,
            _ => NetBase { day, rx, tx },
        };
        let (rx_bps, tx_bps) = match s.net_prev {
            Some((at, prx, ptx)) => {
                let dt = now.duration_since(at).as_secs_f64();
                if dt > 0.0 {
                    (
                        Some((rx.saturating_sub(prx) as f64 / dt).round()),
                        Some((tx.saturating_sub(ptx) as f64 / dt).round()),
                    )
                } else {
                    (None, None)
                }
            }
            None => (None, None),
        };
        snap.net = Some(Net {
            rx_bps,
            tx_bps,
            rx_bytes_today: rx.saturating_sub(base.rx),
            tx_bytes_today: tx.saturating_sub(base.tx),
            rx_bytes_total: rx,
            tx_bytes_total: tx,
        });
        s.net_base = Some(base);
        s.net_prev = Some((now, rx, tx));
    }

    // Per workspace: its cgroup (when isolation made one) and the cached du.
    let ws_root = root.join(CGROUP_PARENT);
    let mut seen = Vec::new();
    for (id, _dir) in workspace_dirs(work_root) {
        let mut u = WorkspaceUsage::default();
        let cg = ws_root.join(Isolation::user_for(&id));
        if cg.is_dir() {
            u.mem_mb = read(cg.join("memory.current"))
                .and_then(|t| parse_cgroup_u64(&t))
                .map(|b| b / MIB);
            u.pids = read(cg.join("pids.current")).and_then(|t| parse_cgroup_u64(&t));
            if let (Some(usage_usec), Some(cores)) = (
                read(cg.join("cpu.stat")).and_then(|t| parse_cpu_stat_usage(&t)),
                snap.cpu.as_ref().map(|c| c.cores),
            ) {
                let prev = s.ws_cpu.get(&id);
                u.cpu_pct = prev.and_then(|p| {
                    p.at.and_then(|at| {
                        cpu_pct(
                            usage_usec.saturating_sub(p.usage_usec),
                            now.duration_since(at).as_micros() as u64,
                            cores,
                        )
                    })
                });
                s.ws_cpu.insert(
                    id.clone(),
                    CpuSample {
                        usage_usec,
                        at: Some(now),
                    },
                );
            }
        }
        if let Some((mb, at)) = s.du.get(&id) {
            u.disk_mb = Some(*mb);
            u.disk_sampled_at = Some(*at);
        }
        seen.push(id.clone());
        snap.workspaces.insert(id, u);
    }
    s.ws_cpu.retain(|k, _| seen.contains(k));
    s.snapshot = snap;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_max_quota_over_period_is_the_core_count() {
        assert_eq!(parse_cpu_max("400000 100000\n"), Some(4.0));
        assert_eq!(parse_cpu_max("150000 100000"), Some(1.5));
        assert_eq!(parse_cpu_max("max 100000"), None);
        assert_eq!(parse_cpu_max(""), None);
    }

    #[test]
    fn cpu_stat_usage_and_pct() {
        let stat =
            "usage_usec 825014943\nuser_usec 459535835\nsystem_usec 365479107\nnr_periods 91103\n";
        assert_eq!(parse_cpu_stat_usage(stat), Some(825_014_943));
        // 4 cores, 5 s: 20 core-seconds available; 4.6 used → 23 %.
        assert_eq!(cpu_pct(4_600_000, 5_000_000, 4.0), Some(23.0));
        assert_eq!(cpu_pct(0, 5_000_000, 4.0), Some(0.0));
        // Never over 100 even when the clock and the counter disagree.
        assert_eq!(cpu_pct(90_000_000, 5_000_000, 4.0), Some(100.0));
        assert_eq!(cpu_pct(1, 0, 4.0), None);
    }

    #[test]
    fn cgroup_numbers_and_max() {
        assert_eq!(parse_cgroup_u64("8589934592\n"), Some(8_589_934_592));
        assert_eq!(parse_cgroup_u64("max\n"), None);
    }

    #[test]
    fn proc_stat_busy_and_total() {
        let text = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 1 2 3 4 5 6 7 8 9 10\n";
        // busy = user+nice+system+irq… = 1000 − idle(800) − iowait(50) = 150
        assert_eq!(parse_proc_stat(text), Some((150, 1000)));
        assert_eq!(parse_proc_stat("intr 1 2 3"), None);
    }

    #[test]
    fn meminfo_is_the_hosts_inside_daytona_but_parses() {
        // Jacob's box, image 0.1.150: 755 GiB total — the host, not the 8 GiB cap.
        let text = "MemTotal:       791899528 kB\nMemFree:        259406448 kB\nMemAvailable:   620145260 kB\n";
        assert_eq!(parse_meminfo(text), Some((791_899_528, 620_145_260)));
        assert_eq!(parse_meminfo("MemTotal: 1 kB"), None);
    }

    #[test]
    fn net_dev_sums_every_interface_but_lo() {
        let text = "Inter-|   Receive                                                |  Transmit\n\
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n\
    lo: 13009976   17990    0    0    0     0          0         0 13009976   17990    0    0    0     0       0          0\n\
  eth0: 221202367  174888    0    0    0     0          0         0 32973249  200227    0    0    0     0       0          0\n\
  eth1: 100 1 0 0 0 0 0 0 200 2 0 0 0 0 0 0\n";
        assert_eq!(parse_net_dev(text), (221_202_467, 32_973_449));
        assert_eq!(parse_net_dev(""), (0, 0));
    }

    #[test]
    fn du_first_field_in_mib() {
        assert_eq!(
            parse_du("3394\t/work/01M5NNT5FNTZNCT3THWNVEW8WM\n"),
            Some(3394)
        );
        assert_eq!(parse_du(""), None);
    }

    #[test]
    fn utc_day_boundary() {
        assert_eq!(utc_day(0), 0);
        assert_eq!(utc_day(86_399_999), 0);
        assert_eq!(utc_day(86_400_000), 1);
    }

    #[test]
    fn snapshot_serialises_without_the_workspace_map_and_omits_unknowns() {
        let snap = Snapshot {
            sampled_at: 5,
            cpu: Some(Cpu {
                cores: 4.0,
                pct: None,
                source: "cgroup",
            }),
            memory: Some(Memory {
                total_mb: 8192,
                used_mb: 1153,
                peak_mb: Some(1204),
                source: "cgroup",
            }),
            load_scope: "host",
            ..Default::default()
        };
        let v = serde_json::to_value(&snap).unwrap();
        assert_eq!(v["cpu"]["cores"], 4.0);
        assert!(v["cpu"].get("pct").is_none());
        assert_eq!(v["memory"]["total_mb"], 8192);
        assert_eq!(v["load_scope"], "host");
        assert!(v.get("workspaces").is_none());
        assert!(v.get("disk").is_none());
    }

    #[test]
    fn workspace_dirs_skip_box_state() {
        let dir = std::env::temp_dir().join(format!("arbos-res-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("01PROJECT")).unwrap();
        std::fs::create_dir_all(dir.join(".arbos")).unwrap();
        std::fs::write(dir.join("stray.txt"), "x").unwrap();
        let ids: Vec<String> = workspace_dirs(&dir).into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, vec!["01PROJECT".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tick_on_this_machine_fills_the_box_fields() {
        let dir = std::env::temp_dir().join(format!("arbos-res-tick-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("01PROJECT")).unwrap();
        tick(&dir);
        // The second sample needs a little wall time for a rate (jiffies at 100 Hz on /proc).
        std::thread::sleep(Duration::from_millis(80));
        tick(&dir);
        let snap = snapshot();
        assert!(snap.sampled_at > 0);
        let disk = snap.disk.as_ref().expect("statvfs of a temp dir");
        assert!(disk.total_mb >= disk.available_mb);
        if cfg!(target_os = "linux") {
            let cpu = snap.cpu.as_ref().expect("cpu from cgroup or /proc");
            assert!(cpu.cores >= 1.0);
            assert!(cpu.pct.is_some_and(|p| (0.0..=100.0).contains(&p)));
            let mem = snap.memory.as_ref().expect("memory");
            assert!(mem.total_mb >= mem.used_mb);
        }
        assert!(snap.workspace("01PROJECT").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
